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

The app-level keys live in one table, `src/keymap.rs` (`NORMAL`): the key handler resolves
keys through it, and the `?` help and Settings → Keyboard are generated from it, so what is
shown can't drift from what the keys do. A new app key is one row there.

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
| `gt` / `gT` | next / previous tab (also `Alt+1…9`; `Tab` / `Shift+Tab` when the screen has no panes) |
| `/` | filter the current list as you type (`Enter` keeps the filter, `Esc` restores it) |
| `i` | start typing (the message box; `Enter` too in a feed) |
| `:` | command palette: every action of the screen plus global commands |
| `Ctrl+p` | go to: a session, project, group or section (fuzzy) |
| `?` | help: every key of this screen (actions, motions, panes, global, leader) |
| `Space` | leader (global, below) |
| `q` | back (quit on the first screen) |

Every other letter is a screen action, listed by `?` and in the palette. Screens never bind
`h j k l g G q : ? /` or `Space` to anything else.

## Leader (`Space`) — which-key menus

| Keys | Action |
| --- | --- |
| `Space Space` | go to a session, project, group or section (same as `Ctrl+p`) |
| `Space c` / `p` / `s` / `h` / `,` | chats / projects / system / agents / settings |
| `Space a` | +actions: the actions of this screen |
| `Space w` | +windows: `h j k l` focus the pane left / below / above / right, `w` next |
| `Space t` | +tabs: `n` / `p` next / previous, `1…9` a tab by its name |
| `Space e` | toggle the sidebar |
| `Space r` | reload |
| `Space H` | switch hub: the saved hubs, or add one (also `:` → Switch hub) |
| `Space L` | log out (a hub with accounts; also `:` → Log out) |
| `Space q` | quit |

`Backspace` goes up a menu, `Esc` closes it. `Ctrl+g` opens the leader everywhere (also in
INSERT, where `Space` types a space).

## Help (`?`)

A panel with every key of the current screen: its actions first, then motions, panes and
tabs, global keys and the leader menu. `j` / `k`, `gg` / `G`, `Ctrl+d` / `Ctrl+u` move, `/`
filters (`Esc` clears), `Enter` on an action — or the action's own key — closes the help and
runs it; `Esc`, `q` or `?` close it.

## Pickers, prompts and confirmations

- Pickers (`:`, `Ctrl+p`, choices) filter fuzzily as you type: every word must appear in
  order, word starts and runs rank first, matched letters are highlighted and the bottom
  border counts the matches. `↑` / `↓`, `Ctrl+n` / `Ctrl+p`, `Ctrl+j` / `Ctrl+k` move,
  `Ctrl+u` clears, `Ctrl+w` deletes a word.
- A filter prompt (`/`, a modal key `filter` or `<what>-filter`) sits at the bottom of the
  screen and updates the list on every key.
- A confirmation of something that can't be undone takes `y` only; `Enter` confirms only
  harmless ones. `n` / `Esc` cancel.

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
  `←` / `→`, `Tab`) pick the agent column, `Enter` connects / disconnects it. `c` opens the
  catalog (an empty search lists popular servers, anything else searches the MCP Registry),
  then asks for the install option, the name, placeholders, env vars / headers (secrets
  masked) and the agents; `a` adds a server by hand, `x` removes it from every agent, `P`
  shows a project's servers too.
- The Skills tab is the matrix of skills × agents: the first column is the skill (`Enter` or
  `v` shows its SKILL.md), `Enter` on an agent column copies the skill there or moves that copy
  to the hub trash; project and plugin copies are read-only. `n` writes a new skill, `i`
  imports skills from a git repository (`owner/repo` or an https URL; an existing different
  skill is replaced only after a confirm), `x` removes a skill from every agent.
- A tab that needs it explains itself in a muted line under the tab bar and in the `?` title.
- Choosing several items (agents, skills to import) uses the picker: `Enter` ticks a row, the
  first row (`Home`) finishes.
- An agent tab has sections: `[` / `]` switch between the overview, skills, rules, agents,
  commands and plugins; `Enter` shows an item, `e` on a rules file opens it in the editor.

## Rules for screens

- Open in NORMAL: no input or terminal takes the keys until `i` / `/` / `Enter`.
- Pane titles are short names; key hints come from `actions()` (`?`, palette) and `hints()`
  (the status line; defaults to the actions, an editor shows its own); `help()` explains the
  screen in a line on top of `?`.
- Lists handle `Home` / `End` (sent by `gg` / `G`) and `PageUp` / `PageDown` (sent by
  `Ctrl+u` / `Ctrl+d`); `kit::list_motion` does it for a `ListState`.
- Filters: `/` opens `ctx.prompt("filter", …)`; the screen gets the text on every key.
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

## Hubs and signing in

The first run (no `--url`, no saved hub) shows the connect screen: a domain or an IP, with
`:port` when it isn't 8787. A bare host tries `https://host` first, then `http://host:8787`;
`host:port` tries https, then http; Enter alone is this machine. Hubs are saved in
`~/.config/hubness/tui.json` (mode 0600) with their username and session token, and the
next start opens the last one. `Space H` (or Settings → Connection) switches hubs, adds one
or forgets one; `--url` overrides the last hub and is saved too.

A hub with accounts shows a sign-in screen (username, masked password; `Ctrl+O` goes to
another hub). When the hub answers 401 the token is dropped and the screen comes back.
`--user <name>` prefills the username. Settings → Users adds and deletes accounts and
changes passwords.
