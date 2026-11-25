//! The Search tab (the web app's SearchView): text across the project's files as you type,
//! with match case / whole word / regex toggles and include / exclude globs; results grouped
//! by file (collapsible); a match opens in the Files tab at its line.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde_json::Value;

use super::{Opener, enc};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Theme, block};

/// Split `text` into (piece, matched) by match ranges given in UTF-16 code units.
pub fn split_ranges(text: &str, ranges: &[(usize, usize)]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = vec![];
    let mut pos = 0usize; // UTF-16 index
    for c in text.chars() {
        let hit = ranges.iter().any(|(s, e)| pos >= *s && pos < *e);
        match out.last_mut() {
            Some((s, h)) if *h == hit => s.push(c),
            _ => out.push((c.to_string(), hit)),
        }
        pos += c.len_utf16();
    }
    out
}

/// A row of the results list: a file header or one of its matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    File { path: String, count: usize, open: bool },
    Match { path: String, line: usize, text: String, ranges: Vec<(usize, usize)> },
}

/// The visible rows: every file, with its matches unless collapsed.
pub fn rows(files: &[Value], collapsed: &HashSet<String>) -> Vec<Hit> {
    let mut out = vec![];
    for f in files {
        let path = f["path"].as_str().unwrap_or_default().to_string();
        let matches = f["matches"].as_array().cloned().unwrap_or_default();
        let open = !collapsed.contains(&path);
        out.push(Hit::File { path: path.clone(), count: matches.len(), open });
        if open {
            for m in matches {
                let ranges = m["ranges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| Some((r[0].as_u64()? as usize, r[1].as_u64()? as usize)))
                    .collect();
                out.push(Hit::Match {
                    path: path.clone(),
                    line: m["line"].as_u64().unwrap_or(1) as usize,
                    text: m["text"].as_str().unwrap_or_default().to_string(),
                    ranges,
                });
            }
        }
    }
    out
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Query,
    Results,
}

pub struct SearchTab {
    id: String,
    opener: Opener,
    query: TextInput,
    case: bool,
    word: bool,
    regex: bool,
    include: String,
    exclude: String,
    files: Vec<Value>,
    total: usize,
    truncated: bool,
    error: Option<String>,
    busy: bool,
    seq: u64,
    collapsed: HashSet<String>,
    rows: Vec<Hit>,
    list: ListState,
    focus: Focus,
}

impl SearchTab {
    pub fn new(id: &str, opener: Opener) -> Self {
        Self {
            id: id.to_string(),
            opener,
            query: TextInput::default(),
            case: false,
            word: false,
            regex: false,
            include: String::new(),
            exclude: String::new(),
            files: vec![],
            total: 0,
            truncated: false,
            error: None,
            busy: false,
            seq: 0,
            collapsed: HashSet::new(),
            rows: vec![],
            list: ListState::default(),
            focus: Focus::Query,
        }
    }

    /// Run the search for the current query and options (a newer one wins).
    fn search(&mut self, ctx: &mut Ctx) {
        self.seq += 1;
        let q = self.query.value();
        if q.trim().is_empty() {
            self.files.clear();
            self.rows.clear();
            self.total = 0;
            self.truncated = false;
            self.error = None;
            self.busy = false;
            return;
        }
        let mut path = format!("/projects/{}/search?q={}", self.id, enc(&q));
        for (on, k) in [(self.regex, "regex"), (self.case, "case"), (self.word, "word")] {
            if on {
                path.push_str(&format!("&{k}=1"));
            }
        }
        if !self.include.trim().is_empty() {
            path.push_str(&format!("&include={}", enc(self.include.trim())));
        }
        if !self.exclude.trim().is_empty() {
            path.push_str(&format!("&exclude={}", enc(self.exclude.trim())));
        }
        self.busy = true;
        ctx.get(format!("search:{}", self.seq), path);
    }

    fn rebuild(&mut self) {
        self.rows = rows(&self.files, &self.collapsed);
        if self.rows.is_empty() {
            self.list.select(None);
        } else {
            let i = self.list.selected().unwrap_or(0).min(self.rows.len() - 1);
            self.list.select(Some(i));
        }
    }

    fn move_by(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let i = self.list.selected().unwrap_or(0) as isize + d;
        self.list.select(Some(i.clamp(0, self.rows.len() as isize - 1) as usize));
    }

    fn toggle_file(&mut self, path: &str) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        self.rebuild();
        if let Some(i) = self.rows.iter().position(|r| matches!(r, Hit::File { path: p, .. } if p == path)) {
            self.list.select(Some(i));
        }
    }

    fn activate(&mut self) {
        match self.list.selected().and_then(|i| self.rows.get(i)).cloned() {
            Some(Hit::File { path, .. }) => self.toggle_file(&path),
            Some(Hit::Match { path, line, .. }) => self.opener.request(&path, Some(line)),
            None => {}
        }
    }

    fn toggle_option(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "C" => self.case = !self.case,
            "W" => self.word = !self.word,
            "X" => self.regex = !self.regex,
            _ => return,
        }
        self.search(ctx);
    }
}

impl Screen for SearchTab {
    fn title(&self) -> String {
        "Search".into()
    }

    fn captures_text(&self) -> bool {
        self.focus == Focus::Query
    }

    fn actions(&self) -> Vec<Action> {
        let on = |b: bool| if b { "on" } else { "off" };
        vec![
            Action::new("/", "Edit the search text"),
            Action::new("C", format!("Match case ({})", on(self.case))),
            Action::new("W", format!("Whole word ({})", on(self.word))),
            Action::new("X", format!("Regular expression ({})", on(self.regex))),
            Action::new("I", "Files to include (globs)"),
            Action::new("E", "Files to exclude (globs)"),
            Action::new("z", "Collapse all files"),
            Action::new("Z", "Expand all files"),
            Action::new("c", "Clear the search"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "/" | "i" => self.focus = Focus::Query,
            "C" | "W" | "X" => self.toggle_option(key, ctx),
            "I" => ctx.prompt("include", "Files to include (e.g. src/**, *.ts)", self.include.clone()),
            "E" => ctx.prompt("exclude", "Files to exclude (e.g. **/*.test.ts)", self.exclude.clone()),
            "z" => {
                self.collapsed = self.files.iter().filter_map(|f| f["path"].as_str().map(String::from)).collect();
                self.list.select(Some(0));
                self.rebuild();
            }
            "Z" => {
                self.collapsed.clear();
                self.rebuild();
            }
            "c" => {
                self.query.set("");
                self.search(ctx);
                self.focus = Focus::Query;
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.focus == Focus::Query {
            // Alt+C / Alt+W / Alt+R toggle the options while typing (as in VS Code)
            if k.modifiers.contains(KeyModifiers::ALT)
                && let KeyCode::Char(c) = k.code
            {
                let key = match c.to_ascii_lowercase() {
                    'c' => "C",
                    'w' => "W",
                    'r' => "X",
                    _ => return Handled::No,
                };
                self.toggle_option(key, ctx);
                return Handled::Yes;
            }
            return match self.query.on_key(k) {
                InputEvent::Changed => {
                    self.search(ctx);
                    Handled::Yes
                }
                InputEvent::Submit => {
                    if !self.rows.is_empty() {
                        self.focus = Focus::Results;
                        if self.list.selected().is_none() {
                            self.list.select(Some(0));
                        }
                    }
                    Handled::Yes
                }
                InputEvent::Cancel if !self.query.is_empty() => {
                    self.query.set("");
                    self.search(ctx);
                    Handled::Yes
                }
                InputEvent::Cancel | InputEvent::Ignored if matches!(k.code, KeyCode::Esc | KeyCode::Down | KeyCode::Tab) => {
                    self.focus = Focus::Results;
                    if self.list.selected().is_none() && !self.rows.is_empty() {
                        self.list.select(Some(0));
                    }
                    Handled::Yes
                }
                _ => Handled::No,
            };
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.list.selected().unwrap_or(0) == 0 {
                    self.focus = Focus::Query;
                } else {
                    self.move_by(-1)
                }
            }
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Enter | KeyCode::Char('o') => self.activate(),
            KeyCode::Left | KeyCode::Right => {
                let path = match self.list.selected().and_then(|i| self.rows.get(i)) {
                    Some(Hit::File { path, .. }) | Some(Hit::Match { path, .. }) => path.clone(),
                    None => return Handled::Yes,
                };
                let open = !self.collapsed.contains(&path);
                let want_open = match k.code {
                    KeyCode::Left => false,
                    KeyCode::Right => true,
                    _ => !open,
                };
                if want_open != open {
                    self.toggle_file(&path);
                }
            }
            KeyCode::Tab => self.focus = Focus::Query,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        self.focus = Focus::Query;
        self.query.paste(text);
        self.search(ctx);
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(n) = key.strip_prefix("search:") {
            if n.parse::<u64>().ok() != Some(self.seq) {
                return; // an older query
            }
            self.busy = false;
            match result {
                Ok(v) => {
                    self.files = v["files"].as_array().cloned().unwrap_or_default();
                    self.total = v["total"].as_u64().unwrap_or(0) as usize;
                    self.truncated = v["truncated"] == true;
                    self.error = None;
                    self.collapsed.clear();
                    self.list.select(if self.files.is_empty() { None } else { Some(0) });
                }
                Err(e) => {
                    self.files.clear();
                    self.error = Some(e.to_string());
                }
            }
            self.rebuild();
            return;
        }
        let Ok(Value::String(v)) = result else { return };
        match key {
            "include" => self.include = v,
            "exclude" => self.exclude = v,
            _ => return,
        }
        self.search(ctx);
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let globs = !self.include.is_empty() || !self.exclude.is_empty();
        let [input, opts, results] =
            Layout::vertical([Constraint::Length(3), Constraint::Length(if globs { 2 } else { 1 }), Constraint::Min(3)]).areas(area);
        let typing = self.focus == Focus::Query;
        self.query.draw(
            f,
            input,
            block("search — Enter results · Alt+C case · Alt+W word · Alt+R regex", typing),
            typing,
            "Search the project",
        );

        let toggle = |on: bool, label: &str| {
            Span::styled(format!(" {label} "), if on { t.key_style() } else { t.muted_style().add_modifier(Modifier::CROSSED_OUT) })
        };
        let summary = if self.query.is_empty() {
            String::new()
        } else if self.busy {
            "searching…".into()
        } else if self.total > 0 {
            format!("{} results in {} files{}", self.total, self.files.len(), if self.truncated { " (not all are shown)" } else { "" })
        } else if self.error.is_none() {
            "no results".into()
        } else {
            String::new()
        };
        let mut lines = vec![Line::from(vec![
            toggle(self.case, "Aa case"),
            Span::raw(" "),
            toggle(self.word, "ab word"),
            Span::raw(" "),
            toggle(self.regex, ".* regex"),
            Span::styled(format!("   {summary}"), if self.truncated { Style::default().fg(t.warn) } else { t.muted_style() }),
        ])];
        if globs {
            lines.push(Line::styled(format!(" include: {}   exclude: {}", self.include, self.exclude), t.muted_style()));
        }
        f.render_widget(Paragraph::new(lines), opts);

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(Line::styled(e.clone(), Style::default().fg(t.danger))).block(block("results", false)), results);
            return;
        }
        if self.query.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled("Type to search across every file of the project.", t.muted_style()))
                    .block(block("results", false)),
                results,
            );
            return;
        }
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| match r {
                Hit::File { path, count, open } => {
                    let (dir, name) =
                        path.rsplit_once('/').map(|(d, n)| (format!("{d}/"), n.to_string())).unwrap_or((String::new(), path.clone()));
                    ListItem::new(Line::from(vec![
                        Span::styled(if *open { "▾ " } else { "▸ " }, t.muted_style()),
                        Span::styled(name, t.bold()),
                        Span::styled(format!("  {dir}"), t.muted_style()),
                        Span::styled(format!("  {count}"), t.accent_style()),
                    ]))
                }
                Hit::Match { line, text, ranges, .. } => {
                    let mut spans = vec![Span::styled(format!("    {line:>5}  "), t.muted_style())];
                    for (piece, hit) in split_ranges(text.trim_end(), ranges) {
                        spans.push(if hit {
                            Span::styled(piece, Style::default().fg(t.warn).add_modifier(Modifier::BOLD | Modifier::UNDERLINED))
                        } else {
                            Span::raw(piece)
                        });
                    }
                    ListItem::new(Line::from(spans))
                }
            })
            .collect();
        let focused = self.focus == Focus::Results;
        let mut state = self.list;
        if !focused {
            state.select(None);
        }
        f.render_stateful_widget(
            List::new(items)
                .block(block("results — Enter open in Files · h / l fold · z / Z fold all", focused))
                .highlight_style(t.selected()),
            results,
            &mut state,
        );
        if focused {
            *self.list.offset_mut() = state.offset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_utf16_ranges() {
        assert_eq!(split_ranges("let foo = 1", &[(4, 7)]), vec![("let ".into(), false), ("foo".into(), true), (" = 1".into(), false)]);
        // "😀" is two UTF-16 units: the match after it starts at 3
        assert_eq!(split_ranges("😀 ab", &[(3, 5)]), vec![("😀 ".into(), false), ("ab".into(), true)]);
    }

    #[test]
    fn rows_fold_files() {
        let files = vec![
            json!({ "path": "a.rs", "matches": [{ "line": 1, "text": "x", "ranges": [[0, 1]] }, { "line": 9, "text": "x", "ranges": [] }] }),
            json!({ "path": "b.rs", "matches": [{ "line": 2, "text": "y", "ranges": [] }] }),
        ];
        assert_eq!(rows(&files, &HashSet::new()).len(), 5);
        let collapsed: HashSet<String> = ["a.rs".to_string()].into();
        let r = rows(&files, &collapsed);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], Hit::File { path: "a.rs".into(), count: 2, open: false });
        assert!(matches!(&r[2], Hit::Match { line: 2, .. }));
    }
}
