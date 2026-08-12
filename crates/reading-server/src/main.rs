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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "reading_server=info,tower_http=warn".into()),
        )
        .init();

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
    std::fs::create_dir_all(&library_dir)?;
    tracing::info!(path = %library_dir.display(), "library directory");

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
