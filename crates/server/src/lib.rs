//! hubness server in Rust: the HTTP API on top of the same SQLite database and the same
//! `HUBNESS_*` settings as the TypeScript server in `apps/server`.
//!
//! The port is incremental: routes not ported yet answer 404, as unknown routes do.

pub mod agents;
pub mod bus;
pub mod catalog;
pub mod config;
pub mod db;
pub mod error;
pub mod home;
pub mod http;
pub mod inbox;
pub mod paths;
pub mod routes;
pub mod sessions;
pub mod workspaces;

use std::sync::{Arc, Mutex};

use axum::Router;
use rusqlite::Connection;
use tower_http::services::{ServeDir, ServeFile};

pub use config::Config;
use error::{ApiResult, HttpError};

/// Shared state of the request handlers.
#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Connection>>,
    config: Arc<Config>,
    bus: bus::Bus,
}

impl AppState {
    /// Prepare folders, open and migrate the database, create the home project.
    pub fn open(config: Config) -> anyhow::Result<Self> {
        config.ensure_dirs()?;
        let db = db::open(&config.db_path)?;
        home::ensure_home_project(&db, &config)?;
        // no agent survives a restart
        db::sessions::recover_interrupted(&db)?;
        Ok(Self { db: Arc::new(Mutex::new(db)), config: Arc::new(config), bus: bus::Bus::default() })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The database connection (held until the guard is dropped).
    fn db(&self) -> ApiResult<std::sync::MutexGuard<'_, Connection>> {
        self.db.lock().map_err(|_| HttpError::new(500, "Database lock poisoned"))
    }

    /// Run synchronous data access with the connection held.
    fn with<T>(&self, f: impl FnOnce(&Connection, &Config) -> ApiResult<T>) -> ApiResult<axum::Json<T>> {
        let db = self.db()?;
        f(&db, &self.config).map(axum::Json)
    }
}

/// The whole app: `/api` plus the built web app (with the SPA fallback to index.html).
pub fn app(state: AppState) -> Router {
    let api = routes::api().layer(axum::middleware::from_fn(http::csrf_guard));
    let mut router = Router::new().nest("/api", api);
    let dist = &state.config.web_dist;
    if dist.is_dir() {
        let spa = ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html")));
        router = router.fallback_service(spa);
    }
    router.with_state(state)
}

/// Bind and serve until SIGINT/SIGTERM.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let state = AppState::open(config)?;
    let cfg = state.config.clone();
    let listener = tokio::net::TcpListener::bind((cfg.host.as_str(), cfg.port)).await?;
    let addr = listener.local_addr()?;
    println!("hubness: http://{}:{}  (data: {})", addr.ip(), addr.port(), cfg.data_dir.display());
    axum::serve(listener, app(state)).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
