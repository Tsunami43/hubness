//! Syntax highlighting (syntect) and diff rendering.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use super::Theme;

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static syntect::highlighting::Theme {
    static T: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    T.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        set.themes.remove("base16-ocean.dark").unwrap_or_else(|| set.themes.into_values().next().expect("a theme"))
    })
}

/// Highlight `code` by a language name or file extension (`rs`, `ts`, `rust`, `README.md`…).
pub fn highlight(code: &str, lang_or_path: &str) -> Vec<Line<'static>> {
    let ss = syntaxes();
    let ext = lang_or_path.rsplit('.').next().unwrap_or(lang_or_path);
    let syntax =
        ss.find_syntax_by_token(lang_or_path).or_else(|| ss.find_syntax_by_extension(ext)).or_else(|| ss.find_syntax_by_name(lang_or_path));
    let Some(syntax) = syntax else { return code.lines().map(|l| Line::raw(l.to_string())).collect() };
    let mut h = HighlightLines::new(syntax, theme());
    code.lines()
        .map(|line| {
            let with_nl = format!("{line}\n");
            match h.highlight_line(&with_nl, ss) {
                Ok(regions) => Line::from(
                    regions
                        .into_iter()
                        .map(|(st, text)| {
                            let mut s = Style::default().fg(Color::Rgb(st.foreground.r, st.foreground.g, st.foreground.b));
                            if st.font_style.contains(FontStyle::BOLD) {
                                s = s.add_modifier(Modifier::BOLD);
                            }
                            if st.font_style.contains(FontStyle::ITALIC) {
                                s = s.add_modifier(Modifier::ITALIC);
                            }
                            Span::styled(text.trim_end_matches('\n').to_string(), s)
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(_) => Line::raw(line.to_string()),
            }
        })
        .collect()
}

/// A unified diff with added/removed/hunk lines colored.
pub fn diff(text: &str) -> Vec<Line<'static>> {
    let t = Theme::get();
    text.lines()
        .map(|l| {
            let style = if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") {
                t.bold()
            } else if l.starts_with('+') {
                Style::default().fg(t.ok)
            } else if l.starts_with('-') {
                Style::default().fg(t.danger)
            } else if l.starts_with("@@") {
                Style::default().fg(t.info)
            } else {
                Style::default()
            };
            Line::styled(l.to_string(), style)
        })
        .collect()
}
