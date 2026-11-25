<p align="center">
  <img src="apps/web/public/brand/logo-hero.svg" alt="hubness" width="320">
</p>

# hubness

A self-hosted web hub for coding agents. Run Claude Code, Codex, OpenCode and Grok on your
own machine or server and work with them from one browser UI — on a desktop or a phone.

## Features

- **Chats with agents** — streaming event feed, tool calls, diffs, file attachments and
  inline permission approvals. Switch agent and model per session.
- **Projects and groups** — clone or create repositories, group several repositories so an
  agent can work across them, keep per-project notes in a `.hub/hub.yaml` manifest.
- **Workspace** — file explorer, CodeMirror editor (with Vim mode), full-text search,
  side panes that agents can open themselves (preview, file, diff) via a built-in MCP tool.
- **Git** — status, staging, commits, branches, history graph, and provider logins for
  pushing to GitHub/GitLab.
- **Run and Preview** — named run commands per project, interactive shells in xterm.js,
  and a preview proxy that makes a dev server reachable from your phone.
- **Sandboxing** — run agents and commands in a bubblewrap sandbox that hides other
  projects, hub data and credentials.
- **Secrets** — encrypted per-project environment variables.
- **Harness** — install, update, sign in and configure the agent CLIs from the UI.
- **System** — Docker containers, systemd units, packages and disks of the host, with an
  "ask about this" button that starts a chat with the relevant logs attached.
- English and Russian UI, light and dark themes, keyboard-driven navigation.

## Requirements

- Node.js 24 or newer
- pnpm 10 (via `corepack enable`)
- git
- At least one agent CLI: `claude`, `codex` (through `@zed-industries/codex-acp`),
  `opencode` or `grok` — the Harness page can install them for you
- Optional: `bwrap` (bubblewrap) for the sandbox environment, Docker and systemd for the
  System page

## Install

`install.sh` builds from this checkout and installs into `~/.cargo/bin`. There are two
flavors:

| Command | Installs | Needs |
| --- | --- | --- |
| `./install.sh full` | `hubcli` — the hub (server with the web app compiled in) — and `hubtui`, plus a systemd user service `hubness` | Rust, Node.js 24, pnpm (or `npx`) |
| `./install.sh tui` | `hubtui` — only the terminal client, no server code | Rust |

```sh
git clone <repo-url> hubness && cd hubness
./install.sh full                 # hub on http://127.0.0.1:8787, started as a user service
./install.sh full --port 9000     # another port; --host sets the address (loopback by default)
./install.sh full --no-service    # no systemd unit: start `hubcli serve` yourself
./install.sh tui                  # just the terminal client
./install.sh full --dry-run       # show every step without changing anything
./install.sh uninstall            # stop the service, remove the binaries (--purge: data too)
```

`full` keeps its settings in `~/.config/hubness/hub.env` (created once with
`HUBNESS_HOST=127.0.0.1`, `HUBNESS_PORT=8787` and the data folder
`~/.local/share/hubness`) and manages the hub with `systemctl --user status|restart hubness`
and `journalctl --user -u hubness`. To upgrade, pull and run the same command again. On a
server, `sudo loginctl enable-linger $USER` keeps the hub running after you log out.

**Remote access.** Run the hub on a server with `./install.sh full` and only the client on
your laptop with `./install.sh tui`. The simplest way to connect keeps the hub on loopback
and forwards its port over SSH:

```sh
ssh -N -L 8787:127.0.0.1:8787 you@server   # in one terminal
hubtui                                    # in another (or: hubtui --url http://127.0.0.1:8787)
```

To listen on a network address instead, see
[Remote access](crates/server/README.md#remote-access) for the authentication the hub
requires, and keep it behind HTTPS or a VPN.

## Quick start (development)

```sh
git clone <repo-url> hubness
cd hubness
corepack enable
pnpm install
cp .env.example .env   # optional, defaults are fine
pnpm dev
```

Open <http://127.0.0.1:5173>. The API server listens on `127.0.0.1:8787`.

Production:

```sh
pnpm build
pnpm start             # serves the UI and API on http://127.0.0.1:8787
```

## Configuration

All settings are environment variables (see [`.env.example`](.env.example)):

| Variable | Default | Purpose |
| --- | --- | --- |
| `HUBNESS_PORT` | `8787` | Server port |
| `HUBNESS_HOST` | `127.0.0.1` | Bind address |
| `HUBNESS_DATA_DIR` | `./.data` | SQLite database, logs, keys |
| `HUBNESS_HOME` | `~/hub` | Hub working folder: `inbox`, `projects`, `home` |
| `HUBNESS_SECRET_KEY` | generated | Key for project secrets (comma-separated for rotation) |
| `HUBNESS_PREVIEW_PORTS` | `8790-8809` | Port range for the preview proxy |
| `HUBNESS_CLAUDE_BIN`, `HUBNESS_OPENCODE_BIN`, `HUBNESS_GROK_BIN` | binary name | Agent CLI paths |
| `HUBNESS_CODEX_ACP` | pinned `npx` command | Codex ACP adapter command |

> **Security.** hubness has no authentication yet and gives agents shell access to the
> host. Keep it on `127.0.0.1`, or put it behind a VPN or an authenticating reverse proxy.
> Never expose it directly to the internet.

## Development

```sh
pnpm dev      # server with auto-restart + Vite dev server
pnpm check    # typecheck all packages
pnpm test     # server tests
```

The repository is a pnpm workspace:

```
apps/server      Hono API, agent runtime, SQLite (node:sqlite)
apps/web         React 19, Vite, Tailwind CSS 4
packages/shared  Shared TypeScript types
crates/          Rust server, API client, TUI; the hubcli and hubtui binaries
```

See [CLAUDE.md](CLAUDE.md) for architecture notes and [CONTRIBUTING.md](CONTRIBUTING.md)
for the workflow.

## License

[MIT](LICENSE)
