//! Building blocks shared by the System, Harness and Settings screens: a screen made of
//! sections (tabs), list navigation, usage bars, a scrollable text view, periodic refresh
//! and small formatters.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, Paragraph};
use serde_json::Value;

use super::tabs::{Tab, Tabs};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::{Theme, block, wrap};

// ---------- a screen of sections ----------

/// A top-level screen made of sections (System, Settings): the tab bar and the active
/// section; `initial` picks the section by id (the route's `Some(section)`).
pub struct Sections {
    title: &'static str,
    tabs: Tabs,
}

impl Sections {
    pub fn new(title: &'static str, tabs: Vec<Tab>, initial: Option<&str>) -> Self {
        let mut tabs = Tabs::new(tabs);
        if let Some(i) = initial.and_then(|id| tabs.tabs.iter().position(|t| t.id == id)) {
            tabs.active = i;
        }
        Self { title, tabs }
    }
}

impl Screen for Sections {
    fn title(&self) -> String {
        format!("{} · {}", self.title, self.tabs.tabs[self.tabs.active].label)
    }
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        self.tabs.draw(f, area, ctx);
    }
    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        self.tabs.on_key(k, ctx)
    }
    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        self.tabs.on_task(key, result, ctx);
    }
    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        self.tabs.on_stream(key, msg, ctx);
    }
    fn actions(&self) -> Vec<Action> {
        self.tabs.actions()
    }

    fn tab_names(&self) -> Vec<String> {
        self.tabs.names()
    }
    fn captures_text(&self) -> bool {
        self.tabs.captures_text()
    }
    fn on_show(&mut self, ctx: &mut Ctx) {
        self.tabs.on_show(ctx);
    }
    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        self.tabs.on_action(key, ctx);
    }
    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        self.tabs.on_paste(text, ctx);
    }
}

// ---------- lists ----------

/// The selection of a list: ↑↓ / j k, PageUp / PageDown, Home / End.
#[derive(Default, Clone)]
pub struct Nav {
    pub state: ListState,
}

impl Nav {
    pub fn selected(&self) -> Option<usize> {
        self.state.selected()
    }

    /// Keep the selection inside a list of `n` items.
    pub fn clamp(&mut self, n: usize) {
        match (n, self.state.selected()) {
            (0, _) => self.state.select(None),
            (_, None) => self.state.select(Some(0)),
            (n, Some(i)) if i >= n => self.state.select(Some(n - 1)),
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        self.state = ListState::default();
    }

    /// A navigation key; `true` when it moved the selection.
    pub fn key(&mut self, k: &KeyEvent, n: usize) -> bool {
        if n == 0 {
            return false;
        }
        let i = self.state.selected().unwrap_or(0);
        let next = match k.code {
            KeyCode::Down | KeyCode::Char('j') => (i + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => i.saturating_sub(1),
            KeyCode::PageDown => (i + 10).min(n - 1),
            KeyCode::PageUp => i.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => n - 1,
            _ => return false,
        };
        self.state.select(Some(next));
        true
    }
}

/// Every word of `needle` occurs in `hay` (case-insensitive); an empty needle matches all.
pub fn matches(hay: &str, needle: &str) -> bool {
    let hay = hay.to_lowercase();
    needle.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

// ---------- bars and formatting ----------

/// Green below 75 %, yellow below 90 %, red above (the web app's bars).
pub fn pct_color(pct: f64) -> Color {
    let t = Theme::get();
    if pct >= 90.0 {
        t.danger
    } else if pct >= 75.0 {
        t.warn
    } else {
        t.ok
    }
}

/// The filled cells of a `width`-cell bar at `pct` percent (0…100, clamped).
pub fn bar_cells(pct: f64, width: usize) -> usize {
    let p = if pct.is_finite() { pct.clamp(0.0, 100.0) } else { 0.0 };
    ((p / 100.0) * width as f64).round() as usize
}

/// A usage bar: `width` cells, colored by `color`.
pub fn bar_with(pct: f64, width: usize, color: Color) -> Vec<Span<'static>> {
    let full = bar_cells(pct, width);
    vec![Span::styled("█".repeat(full), Style::default().fg(color)), Span::styled("░".repeat(width - full), Theme::get().muted_style())]
}

/// A usage bar colored by how full it is.
pub fn bar(pct: f64, width: usize) -> Vec<Span<'static>> {
    bar_with(pct, width, pct_color(pct))
}

/// `5d 3h`, `3h 12m`, `7m` (the web app's uptime format).
pub fn fmt_uptime(sec: u64) -> String {
    let (d, h, m) = (sec / 86_400, (sec % 86_400) / 3600, (sec % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Megabytes as gigabytes with one decimal.
pub fn gb(mb: f64) -> String {
    format!("{:.1}", mb / 1024.0)
}

/// A number as JSON shows it without a trailing `.0` (12, 12.5).
pub fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        Some(f) => format!("{f:.1}"),
        None => "—".into(),
    }
}

/// A string field or a fallback.
pub fn s<'a>(v: &'a Value, fallback: &'a str) -> &'a str {
    v.as_str().unwrap_or(fallback)
}

/// Time until an ISO timestamp: "in 3h 20m", "in 2d 4h", "now".
pub fn until(iso: &str, now_ms: i64) -> String {
    let Some(t) = crate::ui::iso_ms(iso) else { return String::new() };
    let secs = (t - now_ms) / 1000;
    if secs <= 0 {
        return "now".into();
    }
    let secs = secs as u64;
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("in {d}d {h}h")
    } else if h > 0 {
        format!("in {h}h {m}m")
    } else {
        format!("in {}m", m.max(1))
    }
}

/// "just now" / "5m ago" from an ISO timestamp.
pub fn since(iso: &str) -> String {
    match crate::ui::ago(iso).as_str() {
        "" => "—".into(),
        "now" => "just now".into(),
        a => format!("{a} ago"),
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Standard base64 (for the OSC 52 clipboard sequence).
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Put text on the clipboard of the terminal the TUI runs in (OSC 52; works over SSH in
/// most terminals). Also shows it, so it can be selected by hand where OSC 52 is off.
pub fn copy(ctx: &mut Ctx, text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
    ctx.message("Copied to the clipboard", format!("{text}\n\n(if your terminal blocks clipboard access, select the text above)"));
}

/// The `{ok, output}` answer of a system / harness operation, shown in the status line.
pub fn report(ctx: &mut Ctx, what: &str, v: &Value) {
    let out = v["output"].as_str().unwrap_or_default().trim();
    let first = out.lines().last().unwrap_or_default();
    if v["ok"] == false {
        ctx.error(format!("{what}: {}", if first.is_empty() { "failed" } else { first }));
    } else {
        ctx.info(format!("{what}: {}", if first.is_empty() { "OK" } else { first }));
    }
}

// ---------- periodic refresh ----------

/// Re-runs a load every `every` while the screen is visible: a timer task chain that stops
/// by itself once the screen has not been drawn for a while (another screen or tab is open)
/// and starts again on `on_show`.
pub struct Ticker {
    every: Duration,
    running: bool,
    drawn: Instant,
}

impl Ticker {
    pub fn new(every: Duration) -> Self {
        Self { every, running: false, drawn: Instant::now() }
    }

    /// Call from `draw`.
    pub fn drawn(&mut self) {
        self.drawn = Instant::now();
    }

    /// Call from `on_show`: start the chain unless it runs.
    pub fn start(&mut self, ctx: &Ctx, key: &str) {
        self.drawn = Instant::now();
        if !self.running {
            self.running = true;
            self.schedule(ctx, key);
        }
    }

    fn schedule(&self, ctx: &Ctx, key: &str) {
        let every = self.every;
        ctx.task(key.to_string(), async move {
            tokio::time::sleep(every).await;
            Ok(Value::Null)
        });
    }

    /// The timer fired: `true` when the screen is still visible (reload now); the next
    /// tick is scheduled then, otherwise the chain stops.
    pub fn tick(&mut self, ctx: &Ctx, key: &str) -> bool {
        if self.drawn.elapsed() > self.every + Duration::from_secs(3) {
            self.running = false;
            return false;
        }
        self.schedule(ctx, key);
        true
    }
}

// ---------- a scrollable text ----------

/// Read-only text over a list (logs, package info): j k / PageUp PageDown scroll, Esc closes.
pub struct TextView {
    pub title: String,
    /// `None` while loading.
    pub text: Option<String>,
    scroll: usize,
    height: usize,
    lines: usize,
    /// Scrolled to the end on open (logs: the newest lines matter most).
    follow: bool,
}

impl TextView {
    pub fn new(title: impl Into<String>, follow: bool) -> Self {
        Self { title: title.into(), text: None, scroll: 0, height: 1, lines: 0, follow }
    }

    pub fn set(&mut self, text: String) {
        self.text = Some(text);
        self.scroll = if self.follow { usize::MAX } else { 0 };
    }

    /// `Handled::Yes` for scroll keys; Esc / q close (the caller drops the view on `false`).
    pub fn key(&mut self, k: &KeyEvent) -> Option<bool> {
        let max = self.lines.saturating_sub(self.height);
        let page = self.height.max(2) - 1;
        let s = self.scroll.min(max);
        self.scroll = match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Some(false),
            KeyCode::Down | KeyCode::Char('j') => (s + 1).min(max),
            KeyCode::Up | KeyCode::Char('k') => s.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => (s + page).min(max),
            KeyCode::PageUp => s.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => max,
            _ => return None,
        };
        Some(true)
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, hint: &str) {
        let t = Theme::get();
        let b = block(format!("{} — {hint}", self.title), true);
        let inner = b.inner(area);
        let lines: Vec<Line<'static>> = match &self.text {
            None => vec![Line::styled("Loading…", t.muted_style())],
            Some(text) if text.trim().is_empty() => vec![Line::styled("(empty)", t.muted_style())],
            Some(text) => text.lines().flat_map(|l| wrap(Line::raw(l.replace('\t', "    ")), inner.width as usize)).collect(),
        };
        self.height = inner.height as usize;
        self.lines = lines.len();
        let max = self.lines.saturating_sub(self.height);
        self.scroll = self.scroll.min(max);
        let pos = if max > 0 { format!(" {}/{} ", self.scroll + self.height.min(self.lines), self.lines) } else { String::new() };
        let b = b.title_bottom(Line::styled(pos, t.muted_style()).right_aligned());
        f.render_widget(Paragraph::new(lines).scroll((self.scroll as u16, 0)).block(b), area);
    }
}

/// A two-column "label  value" row.
pub fn row(label: &str, value: impl Into<String>, width: usize) -> Line<'static> {
    let t = Theme::get();
    Line::from(vec![Span::styled(format!("  {label:<width$} "), t.muted_style()), Span::raw(value.into())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_colors() {
        assert_eq!(bar_cells(50.0, 10), 5);
        assert_eq!(bar_cells(-5.0, 10), 0);
        assert_eq!(bar_cells(250.0, 10), 10);
        assert_eq!(bar_cells(f64::NAN, 10), 0);
        assert_eq!(pct_color(10.0), Theme::get().ok);
        assert_eq!(pct_color(80.0), Theme::get().warn);
        assert_eq!(pct_color(95.0), Theme::get().danger);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_uptime(59), "0m");
        assert_eq!(fmt_uptime(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(fmt_uptime(5 * 86_400 + 3 * 3600 + 5), "5d 3h");
        assert_eq!(gb(1536.0), "1.5");
        assert_eq!(num(&serde_json::json!(12.0)), "12");
        assert_eq!(num(&serde_json::json!(12.25)), "12.2");
        assert_eq!(num(&Value::Null), "—");
    }

    #[test]
    fn time_until() {
        let now = crate::ui::iso_ms("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(until("2026-01-01T03:20:00Z", now), "in 3h 20m");
        assert_eq!(until("2026-01-03T04:00:00Z", now), "in 2d 4h");
        assert_eq!(until("2025-12-31T00:00:00Z", now), "now");
        assert_eq!(until("garbage", now), "");
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn word_matching() {
        assert!(matches("nginx proxy", ""));
        assert!(matches("Nginx Proxy", "proxy ngi"));
        assert!(!matches("nginx", "redis"));
    }

    #[test]
    fn nav_moves_and_clamps() {
        let mut n = Nav::default();
        n.clamp(3);
        assert_eq!(n.selected(), Some(0));
        n.key(&KeyEvent::from(KeyCode::End), 3);
        assert_eq!(n.selected(), Some(2));
        n.clamp(2);
        assert_eq!(n.selected(), Some(1));
        n.clamp(0);
        assert_eq!(n.selected(), None);
    }
}
