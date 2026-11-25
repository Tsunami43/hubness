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
| `Ctrl+h` / `Ctrl+l` (or `Ctrl+w h/l`) | focus the pane to the left / right |
| `gt` / `gT` | next / previous tab (also `Alt+1…9`) |
| `/` | filter / search the current list |
| `i` | start typing (the message box; `Enter` too in a feed) |
| `:` | command palette: every action of the screen plus global commands |
| `?` | which-key: the keys of this screen |
| `Space` | leader (global, below) |
| `q` | back (quit on the first screen) |

Every other letter is a screen action, listed by `?` and in the palette. Screens never bind
`h j k l g G q : ? /` or `Space` to anything else.

## Leader (`Space`)

| Keys | Action |
| --- | --- |
| `Space Space` | jump to a session (fuzzy) |
| `Space c` | chats |
| `Space p` | projects |
| `Space s` | system |
| `Space h` | harness (agents) |
| `Space ,` | settings |
| `Space e` | toggle the sidebar |
| `Space 1…9` | tab 1…9 |
| `Space r` | reload |
| `Space q` | quit |

`Ctrl+g` opens the leader too (inside an INSERT input, where `Space` types a space).

## Look

- Colors come from `~/.local/state/omarchy/current/theme/colors.toml` (the active Omarchy
  theme) and follow theme switches live. Elsewhere a built-in Tokyo Night palette is used;
  `HUBNESS_TUI_THEME=/path/colors.toml` picks any Omarchy-format palette.
- Rounded borders; the focused pane uses the theme's active border color, others are muted.
- A status line at the bottom: the mode, where you are, messages, running agents, the hub.
- Nerd Font icons (Omarchy ships Nerd Fonts); `HUBNESS_TUI_ICONS=0` turns them off.
