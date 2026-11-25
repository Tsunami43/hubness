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
- **Harness** — install, update, sign in and configure the agent CLIs from the UI; connect MCP
  servers to the agents (written to their own config files) and see their skills, rules,
  subagents and commands.
- **System** — Docker containers, systemd units, packages and disks of the host, with an
  "ask about this" button that starts a chat with the relevant logs attached.
- English and Russian UI, light and dark themes, keyboard-driven navigation.

## Requirements

- Linux (x86_64, aarch64) or macOS (Intel, Apple silicon); `hubcli server` needs systemd
- git
- At least one agent CLI: `claude`, `codex` (through `@zed-industries/codex-acp`, needs
  Node.js), `opencode` or `grok` — the Harness page can install them for you
- Optional: `bwrap` (bubblewrap) for the sandbox environment, Docker and systemd for the
  System page
- For development: Node.js 24+, pnpm 11 (or `npx -y pnpm@11.29.0`; corepack is not needed), Rust 1.91+

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh
```

It downloads the latest release for your OS and CPU, checks its sha256 and puts two
binaries into `~/.local/bin`:

- `hubcli` — the hub (server with the web app inside) and its management commands;
- `hubtui` — the terminal UI, a client for a hub here or on another machine.

On a first install in a terminal it asks where the hub keeps its data (default
`~/.local/share/hubness`), starts it as a service and offers to create an account. Later the
folder is shown in Settings → Storage (web) and Settings → Connection (TUI) and changed with
`hubcli server config data-dir <path>` + `hubcli server restart`.

Options go after `sh -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --tui-only     # just hubtui
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --version 0.2.0
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --from-source  # cargo (+ Node.js for the web app)
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --data-dir /srv/hubness  # no questions
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --no-start     # binaries only
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --uninstall    # data stays
```

`--dir DIR` (or `HUBNESS_INSTALL_DIR`) installs elsewhere. Run it again to upgrade, then
`hubcli server restart`.

## Usage

**Run the hub** as a systemd user service:

```sh
hubcli server start          # http://127.0.0.1:8787, starts again at login (asks for the data folder the first time)
hubcli user add alice        # an account: sign-in for the web app, the TUI and the API
hubcli server status         # running? health, URL, data folder, accounts
hubcli server logs -f        # its log
```

Open the URL in a browser, or run `hubtui`. On a server, `sudo loginctl enable-linger $USER`
keeps the hub running after you log out and across reboots (`server start` reminds you).
Without systemd (macOS, containers) run `hubcli serve` under your own process manager.

**Settings** live in `~/.config/hubness/hub.env`; data (database, logs, keys) in
`~/.local/share/hubness`:

```sh
hubcli server config                        # show host, port, data-dir
hubcli server config port 9000              # then: hubcli server restart
hubcli server start --data-dir ~/hubness/.data   # keep using an existing data folder
```

**The terminal UI.** The first `hubtui` asks for the hub's address — a domain or an IP,
with `:port` when it isn't 8787 (a bare host tries `https://` first, then `http://…:8787`;
Enter alone is this machine). It signs in if the hub has accounts and remembers the hub, so
the next start opens it directly. `Space H` switches between saved hubs or adds one;
`hubtui --url <address>` opens another one from the command line.

**Remote use.** The hub listens on `127.0.0.1` by default. The simplest way in keeps it
there and forwards the port over SSH:

```sh
ssh -N -L 8787:127.0.0.1:8787 you@server    # on your laptop, then: hubtui (address 127.0.0.1)
```

To listen on the network, create an account first and set the host — hubcli refuses a
public address while there are no accounts:

```sh
hubcli user add alice
hubcli server config host 0.0.0.0 && hubcli server restart
```

Plain HTTP is fine on a VPN (Tailscale, WireGuard); across the internet put the hub behind an
HTTPS reverse proxy and connect with `hubtui --url hub.example.com`. See
[Remote access](crates/server/README.md#remote-access).

## Quick start (development)

```sh
git clone <repo-url> hubness
cd hubness
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
| `HUBNESS_MCP_REGISTRY` | `https://registry.modelcontextprotocol.io` | MCP Registry searched by Harness → MCP servers → Catalog |
| `HUBNESS_CLAUDE_BIN`, `HUBNESS_OPENCODE_BIN`, `HUBNESS_GROK_BIN` | binary name | Agent CLI paths |
| `HUBNESS_CODEX_ACP` | pinned `npx` command | Codex ACP adapter command |

> **Security.** hubness gives agents shell access to the host. Keep it on `127.0.0.1`
> unless it has accounts (`hubcli user add`; the TypeScript dev server has none), and put
> it behind HTTPS or a VPN before it leaves your machine.

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
