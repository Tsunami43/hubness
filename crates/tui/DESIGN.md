# hubness TUI — keys and look

The TUI is modal like Vim and takes its colors from the active Omarchy theme.

## Modes

| Mode | When | Shown as |
| --- | --- | --- |
| NORMAL | browsing: lists, feeds, viewers | `NORMAL` |
| INSERT | typing in a message box, a prompt, a filter or a terminal | `INSERT` / `TERMINAL` |
| COMMAND | `:` command line (a fuzzy palette of every action) | `COMMAND` |

`Esc` (or `Ctrl+[`) always goes back to NORMAL: it leaves an input, closes a popup, then
goes back a screen.

## NORMAL mode

Motions work in every list, feed, tree and viewer:

| Keys | Action |
| --- | --- |
| `j` / `k` | down / up (a count works: `5j`) |
| `h` / `l` | left / right: collapse / expand, out of / into a folder, back / open |
| `gg` / `G` | first / last |
| `Ctrl+d` / `Ctrl+u` | half a page down / up |
| `Ctrl+f` / `Ctrl+b` | a page down / up |
| `Enter` | open / confirm |
| `Ctrl+h` / `Ctrl+l`, `Ctrl+w h/j/k/l`, `Space w h/j/k/l` | focus another pane |
| `gt` / `gT` | next / previous tab (also `Alt+1…9`) |
| `/` | filter / search the current list |
| `i` | start typing (the message box; `Enter` too in a feed) |
| `:` | command palette: every action of the screen plus global commands |
| `?` | which-key: the keys of this screen |
| `Space` | leader (global, below) |
| `q` | back (quit on the first screen) |

Every other letter is a screen action, listed by `?` and in the palette. Screens never bind
`h j k l g G q : ? /` or `Space` to anything else.

## Leader (`Space`) — which-key menus

| Keys | Action |
| --- | --- |
| `Space Space` | find a session |
| `Space c` / `p` / `s` / `h` / `,` | chats / projects / system / agents / settings |
| `Space a` | +actions: the keys of this screen (same as `?`) |
| `Space w` | +windows: `h j k l` focus the pane left / below / above / right, `w` next |
| `Space t` | +tabs: `n` / `p` next / previous, `1…9` a tab by its name |
| `Space e` | toggle the sidebar |
| `Space r` | reload |
| `Space q` | quit |

`Backspace` goes up a menu, `Esc` closes it. `Ctrl+g` opens the leader everywhere (also in
INSERT, where `Space` types a space).

## Mouse

- Click the sidebar (sections, sessions), tab pills, key pills such as `A claude-code`, list
  rows (a second click on the selected row opens it), status-line hints and which-key entries.
- The wheel scrolls the list or viewer under the pointer; in the file editor clicks place the
  cursor and dragging selects.
- `HUBNESS_TUI_MOUSE=0` leaves the mouse to the terminal (plain text selection; most terminals
  also select with `Shift` held while mouse capture is on).

## Editing a file

Files tab: open a file, `e` (or `Enter` in the viewer) starts the editor in Vim NORMAL (`EDIT`
in the status line). `i` / `a` / `o` type, `Esc` stops typing, `:w` / `Ctrl+S` save, `:q` or
`Esc` closes, `:q!` discards.

## Harness

- The MCP tab is the matrix of MCP servers × agents: `j` / `k` pick a server, `[` / `]` (or
  `←` / `→`, `Tab`) pick the agent column, `Enter` connects / disconnects it, `a` adds a server,
  `x` removes it from every agent, `P` shows a project's servers too.
- An agent tab has sections: `[` / `]` switch between the overview, skills, rules, agents,
  commands and plugins; `Enter` shows an item, `e` on a rules file opens it in the editor.

## Rules for screens

- Open in NORMAL: no input or terminal takes the keys until `i` / `/` / `Enter`.
- Pane titles are short names; key hints come from `actions()` (`?`, palette) and `hints()`
  (the status line; defaults to the actions, an editor shows its own).
- Clickable regions are registered while drawing with `Ctx::click` / `kit::click_list`.
- Never bind `h j k l g G q : ?` or `Space` as actions; `/` is for filtering.
- Terminals: typing goes to the job while focused, `Ctrl+]` leaves for NORMAL.

## Look

- Colors come from `~/.local/state/omarchy/current/theme/colors.toml` (the active Omarchy
  theme) and follow theme switches live. Elsewhere a built-in Tokyo Night palette is used;
  `HUBNESS_TUI_THEME=/path/colors.toml` picks any Omarchy-format palette.
- Rounded borders; the focused pane uses the theme's active border color, others are muted.
- A status line at the bottom: the mode, where you are, messages, running agents, the hub.
- Nerd Font icons (Omarchy ships Nerd Fonts); `HUBNESS_TUI_ICONS=0` turns them off.
