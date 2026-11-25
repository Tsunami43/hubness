//! Terminal UI for hubness with the web app's features: chats, projects and groups (files,
//! editor, search, git, terminal, run commands, environment), system, harness, settings.
//! A client of the hub's HTTP/SSE API, so it works with a hub on another machine too
//! (`--url`, signing in with a hub account) and shares every session with the web app.

mod app;
pub mod cli;
pub mod core;
mod login;
mod screens;
pub mod ui;

// the file editor (edtui) is used by the project Files tab
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream};
use crossterm::execute;
use edtui as _;
use futures_util::StreamExt;
use hubness_client::Client;
use tokio::sync::mpsc;

use crate::core::AppEvent;

/// Run the TUI against the hub at `url` until the user quits. A hub with accounts gets the
/// sign-in screen unless a stored session is still valid (`user` prefills the username).
pub async fn run(url: &str, user: Option<&str>) -> anyhow::Result<()> {
    let (token, stored_user) = login::stored(url);
    let client = Client::new(url, token.as_deref())?;
    client.health().await.map_err(|e| anyhow::anyhow!("hubness is not reachable at {url}: {e}"))?;
    // a server without accounts (no /auth/status) is open
    let status = client.auth_status().await.ok();
    let signed_in = status.as_ref().and_then(|s| s.user.as_ref()).map(|u| u.username.clone());
    let form = if status.as_ref().is_some_and(|s| s.required) && signed_in.is_none() {
        if token.is_some() {
            login::remember(url, None, None);
            client.set_token(None);
        }
        Some(login::LoginForm::new(user.or(stored_user.as_deref()), None))
    } else {
        None
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let itx = tx.clone();
    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(Ok(e)) = events.next().await {
            if itx.send(AppEvent::Input(e)).is_err() {
                break;
            }
        }
    });

    let mut terminal = ratatui::init();
    // HUBNESS_TUI_MOUSE=0 leaves the mouse to the terminal (plain text selection)
    let mouse = std::env::var("HUBNESS_TUI_MOUSE").map(|v| v != "0").unwrap_or(true);
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    if mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }
    let mut app = app::App::new(client, tx, form, signed_in);
    let result = async {
        loop {
            terminal.draw(|f| app.draw(f))?;
            let Some(ev) = rx.recv().await else { break };
            app.handle(ev);
            // apply everything already queued before redrawing (streaming deltas come in bursts)
            while let Ok(ev) = rx.try_recv() {
                app.handle(ev);
            }
            if app.quit {
                break;
            }
        }
        anyhow::Ok(())
    }
    .await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}
