//! Reading Companion's server.
//!
//!     reading-server
//!
//! Reads DATABASE_URL from the environment or a .env file. Everything else has
//! a default and can be changed from the application once an administrator
//! account exists.

mod auth_layer;
mod error;
mod routes;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use reading_core::db::pg::Db;
use reading_core::dict::Dictionary;
use reading_core::ollama::OllamaClient;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::{AppState, Settings};

/// A page photograph arrives as base64 inside JSON, so the ceiling has to be
/// generous. It still has to exist: without one, a single request can be made
/// arbitrarily large and the server will try to hold all of it.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Load `.env` from beside the executable, then from the working directory.
///
/// That order matters. A Windows service starts in `system32`, not where it
/// was installed, so a deployed server can only find its configuration next
/// to its own binary — and the alternative, machine-wide environment
/// variables, would put the database password where every account on the box
/// can read it.
///
/// Both are tried rather than the first that works, because `dotenvy` never
/// overwrites a variable that is already set: whichever file is read first
/// wins, and the deployed one should. In development there is no `.env` beside
/// `target/debug/`, so the repository's own is found by the second call.
///
/// Returns where it read from, for the startup log. A server that cannot find
/// its configuration otherwise fails several lines later with something that
/// looks unrelated — a permission error on a path nobody chose.
fn load_env() -> Vec<String> {
    let mut loaded = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(".env");
            if beside.exists() && dotenvy::from_path(&beside).is_ok() {
                loaded.push(beside.display().to_string());
            }
        }
    }

    if let Ok(path) = dotenvy::dotenv() {
        loaded.push(path.display().to_string());
    }

    loaded
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_files = load_env();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "reading_server=info,tower_http=warn".into()),
        )
        .init();

    if env_files.is_empty() {
        tracing::info!("no .env found; using the environment as it stands");
    } else {
        for path in &env_files {
            tracing::info!(path = %path, "read configuration");
        }
    }

    let database_url = std::env::var("DATABASE_URL").map_err(|_| {
        anyhow::anyhow!(
            "DATABASE_URL is not set. Run scripts/setup-postgres.ps1, which writes it to .env."
        )
    })?;

    let db = Db::connect(&database_url).await?;
    tracing::info!("database ready");

    // Sessions are refused once expired, so this is housekeeping rather than
    // security — but a table that only grows is a table that eventually
    // matters.
    match db.purge_expired_sessions().await {
        Ok(0) => {}
        Ok(n) => tracing::info!("removed {n} expired session(s)"),
        Err(e) => tracing::warn!(error = %e, "could not purge expired sessions"),
    }

    let settings = Settings::load(&db).await?;
    let ollama = OllamaClient::new(
        settings.ollama_host.clone(),
        Some(&settings.ollama_api_key),
    )
    .with_keep_alive(&settings.keep_alive);
    tracing::info!(host = %settings.ollama_host, "inference server configured");

    let library_dir: PathBuf = std::env::var("LIBRARY_DIR")
        .unwrap_or_else(|_| "library".into())
        .into();

    // Named before it is created. A relative default resolves against the
    // working directory, which for a service is system32 — and the resulting
    // "access denied" says nothing about which path was refused or why it was
    // chosen.
    tracing::info!(path = %library_dir.display(), "library directory");
    std::fs::create_dir_all(&library_dir).map_err(|e| {
        anyhow::anyhow!(
            "could not create the library directory at {}: {e}. \
             Set LIBRARY_DIR to somewhere writable.",
            library_dir.display()
        )
    })?;

    // The application works without the dictionary: hyphenation falls back to
    // a case heuristic and word lookup is unavailable, rather than the server
    // refusing to start over an optional 40MB file.
    let dict_path = std::env::var("DICT_PATH").unwrap_or_else(|_| "dict.sqlite".into());
    let dict = match Dictionary::open(&dict_path) {
        Ok(d) => {
            tracing::info!(path = %dict_path, "dictionary loaded");
            Some(Arc::new(d))
        }
        Err(e) => {
            tracing::warn!(path = %dict_path, error = %e, "dictionary unavailable; lookups disabled");
            None
        }
    };

    let state = AppState {
        db,
        ollama: Arc::new(std::sync::RwLock::new(ollama)),
        settings: Arc::new(std::sync::Mutex::new(settings)),
        library_dir,
        dict,
    };

    let app = routes::router(state)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http());

    // Loopback by default. Binding every interface is a decision with
    // consequences — it is what makes the server reachable from the network —
    // so it is opt-in rather than the thing that happens if you type nothing.
    let bind = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:7878".into());
    let addr: SocketAddr = bind
        .parse()
        .map_err(|e| anyhow::anyhow!("BIND_ADDRESS {bind:?} is not an address: {e}"))?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");

    if addr.ip().is_loopback() {
        tracing::info!("loopback only; set BIND_ADDRESS=0.0.0.0:7878 to accept from the network");
    } else {
        tracing::warn!("reachable from the network. There is no TLS here: put a reverse proxy in front before exposing this beyond a trusted LAN.");
    }

    axum::serve(listener, app).await?;
    Ok(())
}
