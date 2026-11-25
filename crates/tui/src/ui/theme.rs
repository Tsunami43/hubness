//! Colors of the TUI, taken from the active Omarchy theme
//! (`~/.local/state/omarchy/current/theme/colors.toml`) and reloaded when the theme changes,
//! so the TUI looks like the rest of the desktop. Elsewhere a built-in Tokyo Night palette is
//! used; `HUBNESS_TUI_THEME=/path/colors.toml` picks any Omarchy-format palette.

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Instant, SystemTime};

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub bg: Color,
    /// Panels a step darker than the background (sidebar, popups).
    pub bg_dark: Color,
    pub fg: Color,
    /// Secondary text.
    pub dim: Color,
    pub accent: Color,
    pub selection: Color,
    /// Border of the focused pane.
    pub border: Color,
    /// Borders of the other panes.
    pub line: Color,
    pub muted: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub info: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub light: bool,
}

/// Tokyo Night (Omarchy's default theme), for hosts without Omarchy.
const DEFAULT: Theme = Theme {
    bg: Color::Rgb(0x1a, 0x1b, 0x26),
    bg_dark: Color::Rgb(0x16, 0x16, 0x1e),
    fg: Color::Rgb(0xc0, 0xca, 0xf5),
    dim: Color::Rgb(0x56, 0x5f, 0x89),
    accent: Color::Rgb(0x7a, 0xa2, 0xf7),
    selection: Color::Rgb(0x28, 0x34, 0x57),
    border: Color::Rgb(0x7a, 0xa2, 0xf7),
    line: Color::Rgb(0x29, 0x2e, 0x42),
    muted: Color::Rgb(0x56, 0x5f, 0x89),
    ok: Color::Rgb(0x9e, 0xce, 0x6a),
    warn: Color::Rgb(0xe0, 0xaf, 0x68),
    danger: Color::Rgb(0xf7, 0x76, 0x8e),
    info: Color::Rgb(0x7d, 0xcf, 0xff),
    magenta: Color::Rgb(0xbb, 0x9a, 0xf7),
    cyan: Color::Rgb(0x7d, 0xcf, 0xff),
    light: false,
};

struct State {
    theme: Theme,
    source: Option<PathBuf>,
    modified: Option<SystemTime>,
    checked: Instant,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

fn source() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HUBNESS_TUI_THEME")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join(".local/state/omarchy/current/theme/colors.toml");
    p.exists().then_some(p)
}

fn hex(v: &str) -> Option<Color> {
    let h = v.trim().trim_matches('"').strip_prefix('#')?;
    if h.len() < 6 {
        return None;
    }
    let n = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color::Rgb(n(0)?, n(2)?, n(4)?))
}

/// An Omarchy `colors.toml` (flat `key = "#rrggbb"` lines) → a palette.
pub fn parse(text: &str) -> Theme {
    let mut map = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let c = |keys: &[&str], fallback: Color| keys.iter().find_map(|k| map.get(*k).and_then(|v| hex(v))).unwrap_or(fallback);
    let light = map.get("mode").is_some_and(|m| m.contains("light"));
    let base = DEFAULT;
    let bg = c(&["background"], base.bg);
    let muted = c(&["muted", "dark_foreground", "bright_black", "color8"], base.muted);
    Theme {
        bg,
        bg_dark: c(&["dark_background", "darker_background"], bg),
        fg: c(&["foreground"], base.fg),
        dim: c(&["light_foreground", "muted"], base.dim),
        accent: c(&["accent", "blue"], base.accent),
        selection: c(&["selection", "selection_background"], base.selection),
        border: c(&["active_border_color", "accent"], base.border),
        line: muted,
        muted,
        ok: c(&["green"], base.ok),
        warn: c(&["yellow"], base.warn),
        danger: c(&["red"], base.danger),
        info: c(&["cyan", "blue"], base.info),
        magenta: c(&["magenta"], base.magenta),
        cyan: c(&["cyan"], base.cyan),
        light,
    }
}

fn load(path: &Option<PathBuf>) -> (Theme, Option<SystemTime>) {
    match path {
        Some(p) => match (std::fs::read_to_string(p), std::fs::metadata(p).and_then(|m| m.modified())) {
            (Ok(text), Ok(m)) => (parse(&text), Some(m)),
            _ => (DEFAULT, None),
        },
        None => (DEFAULT, None),
    }
}

impl Theme {
    /// The current palette (checks the theme file for changes at most every 2 seconds).
    pub fn get() -> Theme {
        {
            let guard = STATE.read().expect("theme lock");
            if let Some(s) = guard.as_ref()
                && s.checked.elapsed().as_secs() < 2
            {
                return s.theme;
            }
        }
        let mut guard = STATE.write().expect("theme lock");
        let path = source();
        let modified = path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let stale = guard.as_ref().is_none_or(|s| s.source != path || s.modified != modified);
        if stale {
            let (theme, modified) = load(&path);
            *guard = Some(State { theme, source: path, modified, checked: Instant::now() });
        } else if let Some(s) = guard.as_mut() {
            s.checked = Instant::now();
        }
        guard.as_ref().expect("set above").theme
    }

    /// The whole screen: theme background and text.
    pub fn base(&self) -> Style {
        Style::default().bg(self.bg).fg(self.fg)
    }
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }
    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim)
    }
    pub fn line_style(&self) -> Style {
        Style::default().fg(self.line)
    }
    pub fn key_style(&self) -> Style {
        Style::default().fg(self.bg).bg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn selected(&self) -> Style {
        Style::default().bg(self.selection).fg(self.fg).add_modifier(Modifier::BOLD)
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    /// Mark and color of a session status.
    pub fn status(&self, status: &str) -> (char, Color) {
        match status {
            "running" => ('●', self.ok),
            "waiting" => ('?', self.warn),
            "error" => ('!', self.danger),
            _ => ('·', self.muted),
        }
    }
}

/// Nerd Font icons unless `HUBNESS_TUI_ICONS=0`.
pub fn icons() -> bool {
    std::env::var("HUBNESS_TUI_ICONS").map(|v| v != "0").unwrap_or(true)
}

/// An icon, or `fallback` without Nerd Fonts.
pub fn icon(nerd: &'static str, fallback: &'static str) -> &'static str {
    if icons() { nerd } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_omarchy_colors() {
        let t = parse(
            "mode = \"dark\"\naccent = \"#b59790\"\nbackground = \"#0c0b0c\"\nred = \"#c38b7b\"\nactive_border_color = \"#d6d3de\"\n",
        );
        assert_eq!(t.accent, Color::Rgb(0xb5, 0x97, 0x90));
        assert_eq!(t.bg, Color::Rgb(0x0c, 0x0b, 0x0c));
        assert_eq!(t.danger, Color::Rgb(0xc3, 0x8b, 0x7b));
        assert_eq!(t.border, Color::Rgb(0xd6, 0xd3, 0xde));
        assert_eq!(t.ok, DEFAULT.ok, "missing keys fall back");
        assert!(!t.light);
    }
}
