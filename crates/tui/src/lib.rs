//! Terminal UI for hubness with the web app's features: chats, projects and groups (files,
//! editor, search, git, terminal, run commands, environment), system, harness, settings.
//! A client of the hub's HTTP/SSE API, so it works with a hub on another machine too
//! (signing in with a hub account) and shares every session with the web app.
//!
//! Hubs are saved in `~/.config/hubness/tui.json` (see `hubs`): the first run asks for an
//! address, later runs open the last hub, `Space H` switches between them.

mod app;
pub mod cli;
mod connect;
pub mod core;
mod fuzzy;
pub mod hubs;
mod keymap;
mod login;
mod screens;
pub mod ui;

// the file editor (edtui) is used by the project Files tab
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, EventStream, KeyEventKind,
};
use crossterm::execute;
use edtui as _;
use futures_util::StreamExt;
use hubness_client::Client;
use tokio::sync::mpsc;

use crate::app::Next;
use crate::connect::{ConnectEvent, ConnectForm};
use crate::core::AppEvent;

type Term = ratatui::DefaultTerminal;

/// What the TUI shows next.
enum Target {
    /// The connect screen: the address box (`auto` submits it right away) and saved hubs.
    Connect {
        address: String,
        error: Option<String>,
        back: Option<String>,
        auto: bool,
    },
    /// A hub by its URL.
    Hub(String),
    Quit,
}

/// Run the TUI until the user quits. `url` (a URL or an address) overrides the last saved
/// hub; without either the connect screen asks for one. A hub with accounts gets the
/// sign-in screen unless a stored session is still valid (`user` prefills the username).
pub async fn run(url: Option<&str>, user: Option<&str>) -> anyhow::Result<()> {
    let (itx, mut input) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(Ok(e)) = events.next().await {
            if itx.send(e).is_err() {
                break;
            }
        }
    });

    let mut target = match url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => Target::Hub(hubs::key(u)),
        Some(u) => Target::Connect { address: u.to_string(), error: None, back: None, auto: true },
        None => match hubs::Store::load().last {
            Some(last) => Target::Hub(last),
            None => Target::Connect { address: String::new(), error: None, back: None, auto: false },
        },
    };

    let mut terminal = ratatui::init();
    // HUBNESS_TUI_MOUSE=0 leaves the mouse to the terminal (plain text selection)
    let mouse = std::env::var("HUBNESS_TUI_MOUSE").map(|v| v != "0").unwrap_or(true);
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    if mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }
    let mut user = user.map(String::from);
    let result = async {
        loop {
            target = match target {
                Target::Quit => break,
                Target::Connect { address, error, back, auto } => connect(&mut terminal, &mut input, &address, error, back, auto).await?,
                Target::Hub(url) => open_hub(&mut terminal, &mut input, &url, user.take()).await?,
            };
        }
        anyhow::Ok(())
    }
    .await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

/// The connect screen until a hub answers (or the user goes back or quits).
async fn connect(
    terminal: &mut Term,
    input: &mut mpsc::UnboundedReceiver<Event>,
    address: &str,
    error: Option<String>,
    back: Option<String>,
    auto: bool,
) -> anyhow::Result<Target> {
    let store = hubs::Store::load();
    let mut form = ConnectForm::new(address, store.hubs, error, back);
    let mut pending = auto.then(|| address.to_string());
    loop {
        if let Some(address) = pending.take() {
            let shown = if address.is_empty() { "this machine".to_string() } else { address.clone() };
            form.busy = Some(format!("Looking for a hub at {shown}…"));
            terminal.draw(|f| form.draw(f))?;
            match hubs::resolve(&address).await {
                Ok(url) => return Ok(Target::Hub(url)),
                Err(e) => {
                    form.busy = None;
                    form.error = Some(e.to_string());
                }
            }
        }
        terminal.draw(|f| form.draw(f))?;
        let Some(ev) = input.recv().await else { return Ok(Target::Quit) };
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => match form.on_key(k) {
                ConnectEvent::Quit => return Ok(Target::Quit),
                ConnectEvent::Back => return Ok(form.back.take().map_or(Target::Quit, Target::Hub)),
                ConnectEvent::Open(url) => return Ok(Target::Hub(url)),
                ConnectEvent::Connect(address) => pending = Some(address),
                ConnectEvent::None => {}
            },
            Event::Paste(text) => form.paste(&text),
            _ => {}
        }
    }
}

/// Connect to the hub at `url` and run the app on it until the user quits or switches.
async fn open_hub(
    terminal: &mut Term,
    input: &mut mpsc::UnboundedReceiver<Event>,
    url: &str,
    user: Option<String>,
) -> anyhow::Result<Target> {
    let mut store = hubs::Store::load();
    let saved = store.get(url).cloned().unwrap_or_default();
    let mut splash = ConnectForm::new(url, vec![], None, None);
    splash.busy = Some(format!("Connecting to {url}…"));
    terminal.draw(|f| splash.draw(f))?;

    let client = Client::new(url, saved.token.as_deref())?;
    if let Err(e) = hubs::probe(url).await {
        let back = store.last.clone().filter(|l| l != url && store.get(l).is_some());
        let error = format!("{url}: {}", hubs::short_error(&e));
        return Ok(Target::Connect { address: url.to_string(), error: Some(error), back, auto: false });
    }
    store.touch(url);
    store.save();
    // a server without accounts (no /auth/status) is open
    let status = client.auth_status().await.ok();
    let signed_in = status.as_ref().and_then(|s| s.user.as_ref()).map(|u| u.username.clone());
    let form = if status.as_ref().is_some_and(|s| s.required) && signed_in.is_none() {
        if saved.token.is_some() {
            hubs::remember(url, None, None);
            client.set_token(None);
        }
        Some(login::LoginForm::new(user.as_deref().or(saved.username.as_deref()), None))
    } else {
        None
    };

    // a channel per hub: answers to the previous hub's requests can't reach this one
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = app::App::new(client, tx, form, signed_in);
    loop {
        terminal.draw(|f| app.draw(f))?;
        tokio::select! {
            ev = input.recv() => match ev {
                Some(ev) => app.handle(AppEvent::Input(ev)),
                None => break,
            },
            ev = rx.recv() => match ev {
                Some(ev) => app.handle(ev),
                None => break,
            },
        }
        // apply everything already queued before redrawing (streaming deltas come in bursts)
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
        if app.quit {
            break;
        }
    }
    app.shutdown();
    Ok(match app.next.take() {
        None => Target::Quit,
        Some(Next::Hub(next)) => Target::Hub(next),
        Some(Next::Add) => Target::Connect { address: String::new(), error: None, back: Some(url.to_string()), auto: false },
    })
}
