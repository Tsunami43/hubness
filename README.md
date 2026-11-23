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

## Quick start

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
```

See [CLAUDE.md](CLAUDE.md) for architecture notes and [CONTRIBUTING.md](CONTRIBUTING.md)
for the workflow.

## License

[MIT](LICENSE)
