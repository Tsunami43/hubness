//! hubness server in Rust: the HTTP API on top of the same SQLite database and the same
//! `HUBNESS_*` settings as the TypeScript server in `apps/server`.
//!
//! The port is incremental: routes not ported yet answer 404, as unknown routes do.

pub mod agents;
pub mod attachments;
pub mod auth;
pub mod bus;
pub mod catalog;
pub mod config;
pub mod context;
pub mod db;
pub mod environments;
pub mod error;
pub mod extensions;
pub mod files;
pub mod git;
pub mod git_auth;
pub mod git_providers;
pub mod harness;
pub mod home;
pub mod http;
pub mod inbox;
pub mod kanban;
pub mod languages;
mod languages_data;
pub mod manifest;
pub mod mcp;
pub mod paths;
pub mod preview;
pub mod routes;
pub mod runs;
pub mod runtime;
pub mod search;
pub mod secrets;
pub mod sessions;
pub mod system;
pub mod terminal;
pub mod users;
pub mod voice;
pub mod watch;
pub mod web;
pub mod workspaces;

use std::sync::{Arc, Mutex};

use axum::Router;
use rusqlite::Connection;

pub use config::Config;
use error::{ApiResult, HttpError};

/// Shared state of the request handlers.
#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Connection>>,
    config: Arc<Config>,
    bus: bus::Bus,
    runtime: runtime::Runtime,
    watches: watch::Watches,
    /// The hub's own URL for agents' MCP servers (set once the listener is bound).
    self_url: Arc<std::sync::OnceLock<String>>,
    meta: catalog::MetaRefresh,
    harness: harness::Caches,
}

impl AppState {
    /// Prepare folders, open and migrate the database, create the home project.
    pub fn open(config: Config) -> anyhow::Result<Self> {
        config.ensure_dirs()?;
        let db = db::open(&config.db_path)?;
        home::ensure_home_project(&db, &config)?;
        // no agent survives a restart
        db::sessions::recover_interrupted(&db)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(config),
            bus: bus::Bus::default(),
            runtime: runtime::Runtime::default(),
            watches: watch::Watches::default(),
            self_url: Arc::default(),
            meta: Default::default(),
            harness: Default::default(),
        })
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
    // messages (up to 1M characters) and file writes can exceed axum's default 2 MB limit
    let api = routes::api()
        .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(axum::middleware::from_fn(http::csrf_guard))
        .layer(axum::middleware::from_fn(auth::guard));
    let router = web::attach(Router::new().nest("/api", api), &state.config);
    router.with_state(state)
}

/// Bind and serve until SIGINT/SIGTERM.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let state = AppState::open(config)?;
    let cfg = state.config.clone();
    let accounts = users::count(&*state.db.lock().map_err(|_| anyhow::anyhow!("database lock poisoned"))?)?;
    // never open the hub to other machines without accounts
    if !auth::is_loopback(&cfg.host) && accounts == 0 {
        anyhow::bail!("HUBNESS_HOST={} is reachable from other machines: create an account first with `hubness user add <name>`", cfg.host);
    }
    auth::init(state.db.clone())?;
    let listener = tokio::net::TcpListener::bind((cfg.host.as_str(), cfg.port)).await?;
    let addr = listener.local_addr()?;
    let _ = state.self_url.set(format!("http://127.0.0.1:{}", addr.port()));
    println!("hubness: http://{}:{}  (data: {})", addr.ip(), addr.port(), cfg.data_dir.display());
    if accounts > 0 {
        println!("hubness: sign-in required ({accounts} account{})", if accounts == 1 { "" } else { "s" });
        if !auth::is_loopback(&cfg.host) {
            println!("hubness: plain HTTP — use an HTTPS reverse proxy or a VPN across untrusted networks");
        }
    } else {
        println!("hubness: no accounts yet — open to this machine only; create one in the web app or with `hubness user add <name>`");
    }
    let rt = state.runtime.clone();
    axum::serve(listener, app(state).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            rt.stop_all();
            // dev servers and shells must not outlive the hub
            terminal::kill_all_jobs();
        })
        .await?;
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
