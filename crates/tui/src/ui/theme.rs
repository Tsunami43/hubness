//! Colors of the TUI, close to the web app's dark theme (terminal palette colors, so the
//! user's terminal theme still applies).

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub muted: Color,
    pub line: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub info: Color,
}

static THEME: Theme = Theme {
    accent: Color::Cyan,
    muted: Color::DarkGray,
    line: Color::DarkGray,
    ok: Color::Green,
    warn: Color::Yellow,
    danger: Color::Red,
    info: Color::Blue,
};

impl Theme {
    pub fn get() -> &'static Theme {
        &THEME
    }
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }
    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }
    pub fn line_style(&self) -> Style {
        Style::default().fg(self.line)
    }
    pub fn key_style(&self) -> Style {
        Style::default().fg(Color::Black).bg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn selected(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    /// Color of a session status.
    pub fn status(&self, status: &str) -> (char, Color) {
        match status {
            "running" => ('●', self.ok),
            "waiting" => ('?', self.warn),
            "error" => ('!', self.danger),
            _ => ('·', self.muted),
        }
    }
}
