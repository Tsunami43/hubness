<p align="center">
  <img src="apps/web/public/brand/logo-hero.svg" alt="hubness" width="320">
</p>

# hubness

A self-hosted hub for coding agents. Run Claude Code, Codex, OpenCode and Grok on your own
machine or server and work with them from one place — a browser on a desktop or a phone, or
a terminal UI.

## Features

- **Chats with agents** — a live feed with tool calls, diffs, attachments and permission
  approvals; any agent and model per session.
- **Projects and groups** — repositories, and groups of them an agent works across.
- **Workspace** — files, editor (with Vim mode), search, a git client, shells and run commands.
- **Preview** — the app you are building in a built-in browser: device sizes, console, pick an
  element and send it to the agent. Agents have a browser of their own to check their work.
- **SSH connections** — keys the hub holds for your servers; agents use them by name and never
  see a key.
- **Harness** — install, sign in and configure the agent CLIs: MCP servers, skills, rules.
- **System** — Docker, systemd units and packages of the host, with "ask about this".
- **Sandbox, secrets, voice input**, four visual styles, 13 languages.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh
```

One command, `hubness`, lands in `~/.local/bin`: the hub with the web app inside, the
terminal UI and self-update (`hubness update`). Linux x86_64 and Apple silicon have ready
builds; elsewhere add `--from-source`.

## Usage

```sh
hubness server start     # the hub as a service: http://127.0.0.1:8787
hubness webapp           # open it in the browser
hubness tui              # or in the terminal
hubness user add alice   # the hub's account — needed before it listens beyond this machine
```

You need git and at least one agent CLI (`claude`, `codex`, `opencode`, `grok`) — the
Harness page installs them. Optional: bubblewrap for the sandbox, Docker, Chromium for the
agents' browser, OpenSSH for SSH connections.

The [guide](docs/guide.md) covers install options, remote access and every setting.

> **Security.** hubness gives agents shell access to the host. It listens on `127.0.0.1`
> and has one account; put it behind HTTPS or a VPN before it leaves your machine.

## Development

```sh
pnpm install
pnpm dev      # server (:8787) + Vite (:5173)
pnpm check    # typecheck
pnpm test     # server tests
```

Node.js 24+, pnpm 11 and Rust 1.91+. See [CLAUDE.md](CLAUDE.md) for the architecture and
[CONTRIBUTING.md](CONTRIBUTING.md) for the workflow.

## License

[MIT](LICENSE)
