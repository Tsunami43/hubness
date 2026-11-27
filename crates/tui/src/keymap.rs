//! The app-level keys of NORMAL mode in one table: the key handler resolves keys through
//! it and the `?` help, the status line and Settings → Keyboard are generated from it, so
//! what is shown can't drift from what the keys do.
//!
//! A key sequence is a list of tokens: a character (`g`, `G`, `?`), `Space`, a named key
//! (`Down`, `Esc`) or a key with modifiers (`C-d` for Ctrl+d, `M-Left` for Alt+←).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What an app-level key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Down,
    Up,
    Left,
    Right,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    PrevPane,
    NextPane,
    NextTab,
    PrevTab,
    Leader,
    Help,
    Palette,
    Jump,
    /// `q`: back a screen, quit on the first one.
    Back,
    /// `Esc`, Alt+←: back a screen, never quit.
    BackSoft,
    Quit,
}

/// Where a key is listed in the help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Motion,
    Panes,
    Global,
}

pub struct Binding {
    /// Every sequence that runs the command (space-separated tokens); the first ones are
    /// shown in the help.
    pub seqs: &'static [&'static str],
    pub cmd: Cmd,
    pub label: &'static str,
    pub group: Group,
    /// The screen sees the key first (it closes its own popup, leaves an input, switches a
    /// tab); the command runs only when the screen does not use it.
    pub screen_first: bool,
}

const fn b(seqs: &'static [&'static str], cmd: Cmd, label: &'static str, group: Group) -> Binding {
    Binding { seqs, cmd, label, group, screen_first: false }
}

const fn screen_first(seqs: &'static [&'static str], cmd: Cmd, label: &'static str, group: Group) -> Binding {
    Binding { seqs, cmd, label, group, screen_first: true }
}

/// The keys the app handles in NORMAL mode before the screen's own keys.
pub const NORMAL: &[Binding] = &[
    b(&["j", "Down"], Cmd::Down, "down (a count works: 5j)", Group::Motion),
    b(&["k", "Up"], Cmd::Up, "up (a count works: 5k)", Group::Motion),
    b(&["h"], Cmd::Left, "left: fold, out of a folder", Group::Motion),
    b(&["l"], Cmd::Right, "right: unfold, into a folder, open", Group::Motion),
    b(&["g g"], Cmd::Top, "first", Group::Motion),
    b(&["G"], Cmd::Bottom, "last", Group::Motion),
    b(&["C-d"], Cmd::HalfDown, "half a page down", Group::Motion),
    b(&["C-u"], Cmd::HalfUp, "half a page up", Group::Motion),
    b(&["C-f"], Cmd::PageDown, "a page down", Group::Motion),
    b(&["C-b"], Cmd::PageUp, "a page up", Group::Motion),
    b(&["C-h", "C-w h", "C-w k", "C-w C-h", "C-w C-k"], Cmd::PrevPane, "focus the previous pane", Group::Panes),
    b(&["C-l", "C-w l", "C-w j", "C-w w", "C-w C-l", "C-w C-j"], Cmd::NextPane, "focus the next pane", Group::Panes),
    b(&["g t"], Cmd::NextTab, "next tab", Group::Panes),
    b(&["g T"], Cmd::PrevTab, "previous tab", Group::Panes),
    b(&["C-p"], Cmd::Jump, "go to: a session, project, group or section", Group::Global),
    b(&["Space"], Cmd::Leader, "leader menu (Ctrl+g while typing)", Group::Global),
    b(&[":"], Cmd::Palette, "command palette: every action and command", Group::Global),
    b(&["?"], Cmd::Help, "this help", Group::Global),
    screen_first(&["Esc"], Cmd::BackSoft, "close / leave, then back a screen", Group::Global),
    screen_first(&["M-Left"], Cmd::BackSoft, "back a screen", Group::Global),
    screen_first(&["q"], Cmd::Back, "back (quits on the first screen)", Group::Global),
    b(&["C-c"], Cmd::Quit, "quit (also Ctrl+q)", Group::Global),
];

/// Conventions every screen follows; the screens handle these keys themselves.
pub const SCREEN_KEYS: &[(&str, &str, Group)] = &[
    ("Enter", "open / confirm", Group::Motion),
    ("/", "filter the list as you type (Enter keeps, Esc restores)", Group::Motion),
    ("i", "start typing (the message box)", Group::Motion),
    ("Tab", "next pane", Group::Panes),
    ("Alt+Tab / Alt+Shift+Tab", "next / previous tab (also Alt+→ / Alt+←)", Group::Panes),
    ("Alt+1…9", "a tab by its number", Group::Panes),
];

/// The token of a key press; `None` for keys that never start a sequence.
pub fn token(k: &KeyEvent) -> Option<String> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let base = match k.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Down => "Down".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Esc => "Esc".into(),
        _ => return None,
    };
    Some(match (ctrl, alt) {
        (true, _) => format!("C-{base}"),
        (false, true) => format!("M-{base}"),
        _ => base,
    })
}

/// Is a key the app's own in NORMAL mode, so a screen action bound to it would never run?
/// The app looks a key up before the screen's actions: a key that runs a command or starts a
/// longer sequence (`g`) is taken, and a digit starts a count. Keys the screen sees first
/// (`Esc`, `q`) are free for it to handle in `on_key`. The tests hold every screen to it.
#[cfg(test)]
pub fn reserved(key: &str) -> bool {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && key != "0" {
        return true;
    }
    NORMAL.iter().filter(|b| !b.screen_first).flat_map(|b| b.seqs.iter()).any(|seq| seq.split(' ').next() == Some(key))
}

/// The result of looking up the keys typed so far.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// A whole sequence: run this binding.
    Run(&'static Binding),
    /// The start of a longer sequence: wait for the next key.
    Prefix,
    /// Not an app key.
    None,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Binding({:?})", self.cmd)
    }
}

impl PartialEq for Binding {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Binding {}

/// Look up the tokens typed so far in `table`.
pub fn lookup(table: &'static [Binding], typed: &[String]) -> Lookup {
    let mut prefix = false;
    for b in table {
        for seq in b.seqs {
            let toks: Vec<&str> = seq.split(' ').collect();
            if toks.len() == typed.len() && toks.iter().zip(typed).all(|(a, b)| *a == b) {
                return Lookup::Run(b);
            }
            if toks.len() > typed.len() && toks.iter().zip(typed).all(|(a, b)| *a == b) {
                prefix = true;
            }
        }
    }
    if prefix { Lookup::Prefix } else { Lookup::None }
}

/// A token as people read it: `C-d` → `Ctrl+d`, `Down` → `↓`.
pub fn show_token(tok: &str) -> String {
    let named = |s: &str| match s {
        "Down" => "↓".to_string(),
        "Up" => "↑".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        other => other.to_string(),
    };
    if let Some(rest) = tok.strip_prefix("C-") {
        format!("Ctrl+{}", named(rest))
    } else if let Some(rest) = tok.strip_prefix("M-") {
        format!("Alt+{}", named(rest))
    } else {
        named(tok)
    }
}

/// A sequence as people read it: `g g` → `gg`, `C-w h` → `Ctrl+w h`.
pub fn show_seq(toks: &[&str]) -> String {
    if toks.iter().all(|t| t.chars().count() == 1) {
        toks.concat()
    } else {
        toks.iter().map(|t| show_token(t)).collect::<Vec<_>>().join(" ")
    }
}

/// The keys of a binding for the help: its first two sequences.
pub fn show_keys(b: &Binding) -> String {
    b.seqs.iter().take(2).map(|s| show_seq(&s.split(' ').collect::<Vec<_>>())).collect::<Vec<_>>().join(" / ")
}

/// The rows of a help group: the app's bindings, then the screen conventions.
pub fn rows(group: Group) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = NORMAL.iter().filter(|b| b.group == group).map(|b| (show_keys(b), b.label.to_string())).collect();
    out.extend(SCREEN_KEYS.iter().filter(|(_, _, g)| *g == group).map(|(k, l, _)| (k.to_string(), l.to_string())));
    out
}

pub fn group_title(group: Group) -> &'static str {
    match group {
        Group::Motion => "Motions",
        Group::Panes => "Panes and tabs",
        Group::Global => "Global",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn cmd(s: &str) -> Option<Cmd> {
        match lookup(NORMAL, &typed(s)) {
            Lookup::Run(b) => Some(b.cmd),
            _ => None,
        }
    }

    #[test]
    fn sequences_resolve() {
        assert_eq!(cmd("j"), Some(Cmd::Down));
        assert_eq!(cmd("Down"), Some(Cmd::Down));
        assert_eq!(cmd("g g"), Some(Cmd::Top));
        assert_eq!(cmd("g T"), Some(Cmd::PrevTab));
        assert_eq!(cmd("C-w C-h"), Some(Cmd::PrevPane));
        assert_eq!(cmd("C-p"), Some(Cmd::Jump));
        assert_eq!(lookup(NORMAL, &typed("g")), Lookup::Prefix);
        assert_eq!(lookup(NORMAL, &typed("C-w")), Lookup::Prefix);
        assert_eq!(lookup(NORMAL, &typed("g x")), Lookup::None);
        // screen action letters are not app keys
        assert_eq!(lookup(NORMAL, &typed("x")), Lookup::None);
    }

    #[test]
    fn reserved_keys() {
        for k in ["j", "k", "h", "l", "g", "G", ":", "?", "Space", "5", "C-p", "C-w"] {
            assert!(reserved(k), "{k} is the app's");
        }
        // screen-first keys and plain letters are the screen's to use
        for k in ["Esc", "q", "x", "o", "L", "/", "0", "Enter", "]"] {
            assert!(!reserved(k), "{k} is free");
        }
    }

    /// Every `Action::new("<key>", …)` in the crate's sources.
    fn action_keys(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                action_keys(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, _) in text.match_indices("Action::new(\"") {
                    let rest = &text[i + "Action::new(\"".len()..];
                    if let Some(end) = rest.find('"') {
                        out.push((rest[..end].to_string(), path.display().to_string()));
                    }
                }
            }
        }
    }

    /// A screen action on a key the app takes first never runs when the key is pressed
    /// (Settings → Account once had `l` for "log out": `l` is "right"). Checked over the
    /// sources, as a screen's actions depend on its state.
    #[test]
    fn no_screen_action_sits_on_an_app_key() {
        let mut found = vec![];
        action_keys(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.len() > 50, "the scan finds the screens' actions, got {}", found.len());
        for (key, file) in found {
            // this file's own examples
            if file.ends_with("keymap.rs") {
                continue;
            }
            assert!(!reserved(&key), "the action `{key}` in {file} is on an app key: it would never run");
        }
    }

    #[test]
    fn no_sequence_is_bound_twice() {
        let mut seen = std::collections::HashSet::new();
        for b in NORMAL {
            for s in b.seqs {
                assert!(seen.insert(*s), "{s} is bound twice");
            }
        }
    }

    #[test]
    fn a_whole_sequence_never_starts_another() {
        // otherwise the longer one could never be typed
        for b in NORMAL {
            for s in b.seqs {
                let t = typed(s);
                for other in NORMAL.iter().flat_map(|o| o.seqs.iter()) {
                    let o = typed(other);
                    assert!(!(o.len() > t.len() && o[..t.len()] == t[..]), "{s} shadows {other}");
                }
            }
        }
    }

    #[test]
    fn tokens_of_key_presses() {
        let k = |code, m| token(&KeyEvent::new(code, m));
        assert_eq!(k(KeyCode::Char('g'), KeyModifiers::NONE).as_deref(), Some("g"));
        assert_eq!(k(KeyCode::Char('G'), KeyModifiers::SHIFT).as_deref(), Some("G"));
        assert_eq!(k(KeyCode::Char('d'), KeyModifiers::CONTROL).as_deref(), Some("C-d"));
        assert_eq!(k(KeyCode::Char(' '), KeyModifiers::NONE).as_deref(), Some("Space"));
        assert_eq!(k(KeyCode::Left, KeyModifiers::ALT).as_deref(), Some("M-Left"));
        assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE), None);
        // Ctrl wins over Alt; Shift is in the character itself
        assert_eq!(k(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::ALT).as_deref(), Some("C-w"));
        assert_eq!(k(KeyCode::Char('1'), KeyModifiers::ALT).as_deref(), Some("M-1"));
        assert_eq!(k(KeyCode::Esc, KeyModifiers::NONE).as_deref(), Some("Esc"));
        assert_eq!(k(KeyCode::Tab, KeyModifiers::ALT), None);
    }

    #[test]
    fn readable_keys() {
        assert_eq!(show_seq(&["g", "g"]), "gg");
        assert_eq!(show_seq(&["C-w", "h"]), "Ctrl+w h");
        assert_eq!(show_token("M-Left"), "Alt+←");
        let help = rows(Group::Motion);
        assert!(help.iter().any(|(k, l)| k == "j / ↓" && l.starts_with("down")));
        assert!(help.iter().any(|(k, _)| k == "/"));
        // every group has rows
        for g in [Group::Motion, Group::Panes, Group::Global] {
            assert!(!rows(g).is_empty());
        }
    }
}
