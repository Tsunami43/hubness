//! Shared widgets and helpers of the TUI.

pub mod code;
pub mod input;
pub mod markdown;
pub mod term;
pub mod theme;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

pub use theme::Theme;

/// The hubness wordmark, the same pixels as the web app's hero (`logo-hero.svg`): `[hub]ness`
/// in half blocks (one cell = one pixel wide, two tall, so the pixels come out square).
pub const LOGO_PIXELS: [&str; 4] = [
    "█▀▀ █           █     ▀▀█",
    "█   █▄▀▀▄ █   █ █▄▀▀▄   █ █▄▀▀▄ ▄▀▀▀▄ ▄▀▀▀▀ ▄▀▀▀▀",
    "█   █   █ █  ▄█ █   █   █ █   █ █▀▀▀▀  ▀▀▀▄  ▀▀▀▄",
    "▀▀▀ ▀   ▀  ▀▀ ▀ ▀▀▀▀  ▀▀▀ ▀   ▀  ▀▀▀  ▀▀▀▀  ▀▀▀▀",
];

/// The block cursor after the wordmark (4×7 pixels in the web hero).
const LOGO_CURSOR: [&str; 4] = ["████", "████", "████", "▀▀▀▀"];

/// Rows the header takes (prompt, wordmark, rule, gap, tagline).
pub const LOGO_HEIGHT: u16 = 1 + 1 + LOGO_PIXELS.len() as u16 + 1 + 1 + 1;

/// The brand greens of the web hero (the wordmark keeps them on every theme).
const LOGO_BRIGHT: ratatui::style::Color = ratatui::style::Color::Rgb(0x39, 0xff, 0x7a);
const LOGO_DIM: ratatui::style::Color = ratatui::style::Color::Rgb(0x1f, 0x9e, 0x4c);

/// The header of the home, connect and sign-in screens: `> connect --all agents`, the
/// wordmark with its cursor, a rule and the tagline — as on the web app's home screen.
pub fn logo() -> ratatui::widgets::Paragraph<'static> {
    let t = Theme::get();
    let width = LOGO_PIXELS.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 1 + LOGO_CURSOR[0].chars().count();
    // every line padded to the same width: centered as a block, left edges stay aligned
    let pad = |text: String| format!("{text:<width$}");
    let bold = Style::default().fg(LOGO_BRIGHT).add_modifier(ratatui::style::Modifier::BOLD);
    let mut lines = vec![Line::raw(""), Line::styled(pad("> connect --all agents".into()), Style::default().fg(LOGO_DIM))];
    for (i, row) in LOGO_PIXELS.iter().enumerate() {
        let gap = width - row.chars().count() - LOGO_CURSOR[i].chars().count();
        lines.push(Line::from(vec![
            Span::styled(row.to_string(), bold),
            Span::raw(" ".repeat(gap)),
            Span::styled(LOGO_CURSOR[i], Style::default().fg(LOGO_BRIGHT)),
        ]));
    }
    lines.push(Line::styled("─".repeat(width), Style::default().fg(LOGO_DIM)));
    lines.push(Line::raw(""));
    lines.push(Line::styled("One hub. Any agent.", t.muted_style()));
    ratatui::widgets::Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center)
}

/// A bordered block with a title; highlighted when focused. A column of air on each side
/// keeps text off the frame (`inner()` accounts for it).
pub fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    let t = Theme::get();
    let title = title_only(&title.into());
    let b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .border_style(if focused { Style::default().fg(t.border) } else { t.line_style() });
    if title.is_empty() {
        return b;
    }
    let style = if focused { Style::default().fg(t.accent).add_modifier(ratatui::style::Modifier::BOLD) } else { t.dim_style() };
    b.title(Span::styled(format!(" {title} "), style))
}

/// A pane title without the key hints screens append after " — " (hints are shown in the
/// status line and by `?`), with the first letter capitalized.
pub fn title_only(raw: &str) -> String {
    let name = raw.split(" — ").next().unwrap_or(raw).trim();
    // paths and file names keep their case
    let first = name.split_whitespace().next().unwrap_or("");
    if first.contains('/') || first.contains('.') {
        return name.to_string();
    }
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// A rectangle of `w` × `h` centered in `area` (clamped to it).
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [v] = Layout::vertical([Constraint::Length(h.min(area.height))]).flex(Flex::Center).areas(area);
    let [r] = Layout::horizontal([Constraint::Length(w.min(area.width))]).flex(Flex::Center).areas(v);
    r
}

/// Hard-wrap a styled line to `width` columns (counting chars), keeping span styles.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let total: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if total <= width {
        return vec![line];
    }
    let mut out = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for span in line.spans {
        let mut rest: Vec<char> = span.content.chars().collect();
        while !rest.is_empty() {
            let take = (width - used).min(rest.len());
            let piece: String = rest.drain(..take).collect();
            cur.push(Span::styled(piece, span.style));
            used += take;
            if used == width {
                out.push(Line::from(std::mem::take(&mut cur)));
                used = 0;
            }
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    out
}

/// The rows shown of `len` (already wrapped) rows: the last `height`, `scroll` rows up
/// from the end.
pub fn tail_range(len: usize, height: usize, scroll: usize) -> std::ops::Range<usize> {
    let max = len.saturating_sub(height);
    let offset = max.saturating_sub(scroll.min(max));
    offset..(offset + height).min(len)
}

/// A short relative time ("5m", "3h", "2d") from an ISO timestamp.
pub fn ago(iso: &str) -> String {
    let Ok(t) = chrono_lite(iso) else { return String::new() };
    let secs = (now_ms() - t) / 1000;
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Milliseconds since the epoch of an ISO timestamp the hub wrote.
pub fn iso_ms(iso: &str) -> Option<i64> {
    chrono_lite(iso).ok()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Milliseconds since the epoch of `YYYY-MM-DDTHH:MM:SS(.mmm)Z` (what the hub writes).
fn chrono_lite(iso: &str) -> Result<i64, ()> {
    let b = iso.as_bytes();
    if b.len() < 19 {
        return Err(());
    }
    let n = |r: std::ops::Range<usize>| iso.get(r).and_then(|s| s.parse::<i64>().ok()).ok_or(());
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let ms = if b.get(19) == Some(&b'.') { iso.get(20..23).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) } else { 0 };
    // days from civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Ok(((days * 86_400 + h * 3600 + mi * 60 + s) * 1000) + ms)
}

/// One line of "key label" hints.
pub fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let t = Theme::get();
    let mut spans = vec![];
    for (k, l) in pairs {
        spans.push(Span::styled(format!(" {k} "), t.key_style()));
        spans.push(Span::styled(format!(" {l}  "), t.muted_style()));
    }
    Line::from(spans)
}

pub fn style_fg(c: ratatui::style::Color) -> Style {
    Style::default().fg(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(chrono_lite("1970-01-01T00:00:01.500Z"), Ok(1500));
        assert_eq!(chrono_lite("2026-11-25T00:00:00.000Z").map(|ms| ms / 86_400_000), Ok(20_782));
    }

    #[test]
    fn titles_lose_hints_but_keep_paths() {
        assert_eq!(title_only("feed — j/k scroll"), "Feed");
        assert_eq!(title_only("src/main.rs (changes)"), "src/main.rs (changes)");
        assert_eq!(title_only("README.md · e edit"), "README.md · e edit");
    }

    #[test]
    fn wraps() {
        assert_eq!(wrap(Line::raw("abcdefghij"), 4).len(), 3);
        assert_eq!(tail_range(3, 2, 0), 1..3);
        assert_eq!(tail_range(3, 2, 5), 0..2);
        assert_eq!(tail_range(1, 5, 0), 0..1);
    }
}
