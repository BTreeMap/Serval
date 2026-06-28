//! Serval binary entry point.
//!
//! Boots the two planes — the Control Plane (management API + embedded UI) and
//! the Data Plane (public delivery) — over a shared PostgreSQL pool and a
//! single in-memory delivery cache, or runs an offline admin-allowlist command.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpSocket};
use tokio::signal;

use serval::auth::AuthService;
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

    // Bind one delivery socket per Tokio worker thread with SO_REUSEPORT.
    // The kernel distributes incoming SYNs across all listening sockets,
    // eliminating the single accept-loop bottleneck: each worker thread gets
    // its own kernel-side accept queue and accepts connections independently.
    // Uses tokio::net::TcpSocket directly — no socket2 dependency needed.
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let delivery_addr: SocketAddr = config.data_plane_addr;
    let mut delivery_listeners = Vec::with_capacity(worker_threads);
    for _ in 0..worker_threads {
        let sock = match delivery_addr {
            SocketAddr::V4(_) => TcpSocket::new_v4(),
            SocketAddr::V6(_) => TcpSocket::new_v6(),
        }
        .context("tokio: create delivery socket")?;
        sock.set_reuseport(true)
            .context("tokio: SO_REUSEPORT on delivery socket")?;
        sock.set_reuseaddr(true)
            .context("tokio: SO_REUSEADDR on delivery socket")?;
        sock.bind(delivery_addr)
            .with_context(|| format!("tokio: bind delivery socket to {delivery_addr}"))?;
        let listener = sock
            .listen(65535)
            .context("tokio: listen on delivery socket")?;
        delivery_listeners.push(listener);
    }

    tracing::info!(
        control_plane = %config.control_plane_addr,
        data_plane = %config.data_plane_addr,
        delivery_listeners = delivery_listeners.len(),
        "serval is listening"
    );

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
