//! Serval binary entry point.
//!
//! Boots the two planes — the Control Plane (management API + embedded UI) and
//! the Data Plane (public delivery) — over a shared PostgreSQL pool and a
//! single in-memory delivery cache, or runs an offline admin-allowlist command.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::signal;

use serval::auth::{AuthConfig, AuthService};
use serval::cache::DeliveryCache;
use serval::config::Config;
use serval::crypto;
use serval::db::{self, Repository};
use serval::state::{ControlState, DeliveryState};
use serval::{api, delivery};

/// High-performance snippet delivery and templating service.
#[derive(Parser)]
#[command(name = "serval", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run both planes (the default when no subcommand is given).
    Serve,
    /// Manage the local admin allowlist.
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    /// Grant a user the admin role.
    Promote {
        /// The user's stable identity (the token `sub`, or the dev superuser).
        user_id: String,
    },
    /// Revoke a user's admin role.
    Demote {
        /// The user's stable identity.
        user_id: String,
    },
    /// List every user currently holding the admin role.
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let config = Config::from_env().context("failed to load configuration")?;
    let pool = db::connect(&config.database_url, config.database_max_connections)
        .await
        .context("failed to connect to PostgreSQL")?;
    let repo = Repository::new(pool);

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config, repo).await,
        Command::Admin { action } => run_admin(&repo, action).await,
    }
}
/// Boot both planes and run until a shutdown signal arrives.
async fn serve(config: Config, repo: Repository) -> Result<()> {
    // Log every non-secret setting before any field is consumed so operators
    // can confirm the active configuration from a single log bundle.
    log_startup_config(&config);
    // One cache handle is shared by both planes: a Control Plane write evicts
    // exactly what a Data Plane read would load.
    let cache = DeliveryCache::new(config.cache_byte_budget);
    // One signer derived from the deployment secret: the Control Plane mints
    // signed ids, the Data Plane verifies them. Both must share the same key.
    let signer = crypto::IdSigner::new(&config.id_secret);
    let auth = std::sync::Arc::new(
        AuthService::new(config.auth)
            .await
            .context("failed to initialize the auth service")?,
    );

    // The public Data Plane reads through its own pool pinned to read-only
    // transactions, so even a total compromise of the public plane cannot
    // mutate storage — the write-capable pool stays exclusive to the Control
    // Plane.
    let delivery_pool =
        db::connect_read_only(&config.database_url, config.database_max_connections)
            .await
            .context("failed to connect the read-only data plane pool")?;
    let delivery_repo = Repository::new(delivery_pool);

    let control_state = ControlState {
        repo: repo.clone(),
        cache: cache.clone(),
        auth,
        signer: signer.clone(),
        data_plane_url: config.data_plane_url.as_deref().map(std::sync::Arc::from),
    };
    let delivery_state = DeliveryState {
        repo: delivery_repo,
        cache,
        signer,
    };

    let control_app = api::router(control_state);
    let delivery_app = delivery::router(delivery_state);

    let control_listener = TcpListener::bind(config.control_plane_addr)
        .await
        .with_context(|| {
            format!(
                "failed to bind control plane on {}",
                config.control_plane_addr
            )
        })?;

    // Bind the delivery plane listeners. On Unix, SO_REUSEPORT gives N
    // parallel accept queues (one per CPU). On other platforms a single
    // listener is used instead — see bind_delivery_listeners.
    let delivery_addr: SocketAddr = config.data_plane_addr;
    let delivery_listeners = bind_delivery_listeners(delivery_addr)?;

    tracing::info!("serval is listening");

    let control = axum::serve(control_listener, control_app)
        .with_graceful_shutdown(shutdown_signal("control plane"));

    // Spawn one delivery server task per SO_REUSEPORT socket.
    let delivery_handles: Vec<_> = delivery_listeners
        .into_iter()
        .map(|listener| {
            let app = delivery_app.clone();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown_signal("data plane"))
                    .await
                    .expect("delivery plane server error");
            })
        })
        .collect();

    tokio::try_join!(
        async { control.await.context("control plane server error") },
        async {
            for h in delivery_handles {
                h.await.context("delivery task panicked")?;
            }
            Ok(())
        },
    )?;

    tracing::info!("serval shut down cleanly");
    Ok(())
}

/// Bind the delivery plane TCP listeners.
///
/// On Unix platforms where `SO_REUSEPORT` is available (Linux, macOS,
/// FreeBSD, …) binds one socket per available CPU. The kernel distributes
/// incoming SYNs across all sockets so each Tokio worker thread has its own
/// accept queue, eliminating the single accept-loop bottleneck.
///
/// On platforms that do not support `SO_REUSEPORT` (Windows, Solaris,
/// illumos, Cygwin) falls back gracefully to a single standard listener so
/// the server still starts correctly — without the throughput optimisation.
fn bind_delivery_listeners(addr: SocketAddr) -> Result<Vec<TcpListener>> {
    #[cfg(all(
        unix,
        not(target_os = "solaris"),
        not(target_os = "illumos"),
        not(target_os = "cygwin")
    ))]
    {
        use tokio::net::TcpSocket;
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);
        tracing::info!(
            addr = %addr,
            listeners = n,
            "data plane: SO_REUSEPORT — binding {n} parallel accept queues"
        );
        let mut listeners = Vec::with_capacity(n);
        for _ in 0..n {
            let sock = match addr {
                SocketAddr::V4(_) => TcpSocket::new_v4(),
                SocketAddr::V6(_) => TcpSocket::new_v6(),
            }
            .context("create delivery socket")?;
            sock.set_reuseport(true)
                .context("SO_REUSEPORT on delivery socket")?;
            sock.set_reuseaddr(true)
                .context("SO_REUSEADDR on delivery socket")?;
            sock.bind(addr)
                .with_context(|| format!("bind delivery socket to {addr}"))?;
            listeners.push(sock.listen(65535).context("listen on delivery socket")?);
        }
        Ok(listeners)
    }
    // Fallback for platforms without SO_REUSEPORT (Windows, Solaris, …).
    #[cfg(not(all(
        unix,
        not(target_os = "solaris"),
        not(target_os = "illumos"),
        not(target_os = "cygwin")
    )))]
    {
        tracing::warn!(
            addr = %addr,
            "data plane: SO_REUSEPORT unavailable on this platform — \
             using a single listener (reduced throughput at high concurrency)"
        );
        let std_listener = std::net::TcpListener::bind(addr)
            .with_context(|| format!("failed to bind data plane on {addr}"))?;
        std_listener
            .set_nonblocking(true)
            .context("failed to set delivery socket to nonblocking")?;
        Ok(vec![
            TcpListener::from_std(std_listener)
                .context("failed to create tokio delivery listener")?,
        ])
    }
}

/// Log all non-secret configuration fields before any are consumed.
///
/// Secrets (`DATABASE_URL`, `ID_SIGNING_SECRET`) are deliberately omitted.
/// Every setting that has a non-obvious default, or that silently degrades
/// security when left at its default, is logged at `WARN` level; everything
/// else is `INFO`.
fn log_startup_config(config: &Config) {
    // ── Network ─────────────────────────────────────────────────────────
    tracing::info!(
        control_plane = %config.control_plane_addr,
        data_plane    = %config.data_plane_addr,
        "network addresses"
    );

    match &config.data_plane_url {
        Some(url) => tracing::info!(url = %url, "data plane public URL"),
        None => tracing::warn!(
            "DATA_PLANE_PUBLIC_URL unset — dashboard will infer data plane \
             origin from browser location (fine for same-host deployments)"
        ),
    }

    // ── Database ─────────────────────────────────────────────────────────
    tracing::info!(
        max_connections = config.database_max_connections,
        "database pool"
    );

    // ── Cache ────────────────────────────────────────────────────────────
    let budget_mib = config.cache_byte_budget / (1024 * 1024);
    tracing::info!(
        byte_budget = config.cache_byte_budget,
        budget_mib = budget_mib,
        "delivery cache"
    );

    // ── Auth ─────────────────────────────────────────────────────────────
    match &config.auth {
        AuthConfig::None => tracing::warn!(
            "auth=none: all requests are trusted as a dev superuser; \
             never use this mode in production"
        ),
        AuthConfig::Oauth(s) => tracing::info!(
            issuer             = %s.issuer,
            audience           = %s.audience,
            scopes             = %s.scopes,
            jwks_url           = %s.jwks_url,
            jwks_cache_ttl_s   = s.jwks_cache_ttl.as_secs(),
            "auth=oauth"
        ),
        AuthConfig::Cloudflare(s) => tracing::info!(
            team_domain        = %s.team_domain,
            audience           = %s.audience,
            certs_cache_ttl_s  = s.certs_cache_ttl.as_secs(),
            "auth=cloudflare"
        ),
    }
}

/// Execute an offline admin-allowlist command.
async fn run_admin(repo: &Repository, action: AdminAction) -> Result<()> {
    match action {
        AdminAction::Promote { user_id } => {
            repo.set_admin(&user_id, true)
                .await
                .context("failed to grant admin role")?;
            println!("Granted admin role to {user_id}.");
        }
        AdminAction::Demote { user_id } => {
            repo.set_admin(&user_id, false)
                .await
                .context("failed to revoke admin role")?;
            println!("Revoked admin role from {user_id}.");
        }
        AdminAction::List => {
            let admins = repo.list_admins().await.context("failed to list admins")?;
            if admins.is_empty() {
                println!("No users currently hold the admin role.");
            } else {
                println!("Admins ({}):", admins.len());
                for admin in admins {
                    println!("  {}", admin.id);
                }
            }
        }
    }
    Ok(())
}

/// Initialize structured logging, honoring `RUST_LOG` with an `info` default.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}

/// Resolve once either Ctrl-C or (on Unix) SIGTERM is received.
async fn shutdown_signal(plane: &str) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!(plane, "shutdown signal received; draining connections");
}
