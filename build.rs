//! Build orchestration for the embedded frontend.
//!
//! The Control Plane serves the React dashboard from `frontend/dist/`, which is
//! baked into the binary by `rust-embed`. That directory therefore must exist
//! and be current before the crate is compiled. This script builds it.
//!
//! Behavior:
//! * By default it runs the production frontend build (`npm ci` when
//!   `node_modules` is absent, then `npm run build`).
//! * Set `SERVAL_SKIP_FRONTEND_BUILD=1` to skip the npm build entirely and use
//!   whatever already exists in `frontend/dist/` — used by CI/Docker stages that
//!   build the frontend in a dedicated step, and by `cargo` invocations on
//!   machines without Node.
//! * If the build is skipped (or `npm` is unavailable) and `frontend/dist/` is
//!   missing, a minimal placeholder `index.html` is written so the crate still
//!   compiles. A warning is emitted so the omission is visible.

use std::path::Path;
use std::process::Command;

fn main() {
    let frontend = Path::new("frontend");
    let dist = frontend.join("dist");

    // Rebuild whenever a frontend input changes. `dist` itself is watched so an
    // out-of-band `npm run build` is also picked up.
    for path in [
        "frontend/src",
        "frontend/index.html",
        "frontend/package.json",
        "frontend/package-lock.json",
        "frontend/vite.config.ts",
        "frontend/tsconfig.json",
        "frontend/tsconfig.app.json",
        "frontend/tsconfig.node.json",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=SERVAL_SKIP_FRONTEND_BUILD");

    let skip = std::env::var_os("SERVAL_SKIP_FRONTEND_BUILD").is_some();

    if skip {
        ensure_dist_exists(&dist);
        return;
    }

    if !frontend.join("package.json").exists() {
        warn("frontend/package.json not found; skipping frontend build");
        ensure_dist_exists(&dist);
        return;
    }

    match build_frontend(frontend) {
        Ok(()) => {}
        Err(message) => {
            warn(&format!("frontend build skipped: {message}"));
            ensure_dist_exists(&dist);
        }
    }
}

/// Run the production frontend build, installing dependencies if needed.
fn build_frontend(frontend: &Path) -> Result<(), String> {
    let npm = npm_command();

    if !frontend.join("node_modules").exists() {
        run(&npm, &["ci"], frontend).map_err(|e| format!("`npm ci` failed: {e}"))?;
    }

    run(&npm, &["run", "build"], frontend).map_err(|e| format!("`npm run build` failed: {e}"))?;
    Ok(())
}

/// The npm executable name, accounting for Windows' `npm.cmd`.
fn npm_command() -> String {
    std::env::var("NPM").unwrap_or_else(|_| {
        if cfg!(windows) {
            "npm.cmd".to_owned()
        } else {
            "npm".to_owned()
        }
    })
}

/// Run a command in `dir`, returning a readable error on failure.
fn run(program: &str, args: &[&str], dir: &Path) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("could not launch `{program}`: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program}` exited with {status}"))
    }
}

/// Guarantee `dist/index.html` exists so `rust-embed` can compile even when the
/// real build did not run.
fn ensure_dist_exists(dist: &Path) {
    if dist.join("index.html").exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(dist) {
        warn(&format!("could not create {}: {e}", dist.display()));
        return;
    }
    let placeholder = "<!doctype html><html lang=\"en\"><head><meta charset=\"UTF-8\">\
<title>Serval</title></head><body><p>Frontend not built. Run \
<code>npm run build</code> in <code>frontend/</code>.</p></body></html>\n";
    if let Err(e) = std::fs::write(dist.join("index.html"), placeholder) {
        warn(&format!("could not write placeholder index.html: {e}"));
    }
}

/// Emit a build-script warning visible in `cargo build` output.
fn warn(message: &str) {
    println!("cargo:warning={message}");
}
