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
    ("Tab / Shift+Tab", "next / previous pane; next / previous tab on a screen without panes", Group::Panes),
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
