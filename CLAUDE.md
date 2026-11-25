# CLAUDE.md

Guidance for Claude Code (and other coding agents) working in this repository.

## What hubness is

hubness is a self-hosted web hub for running coding agents on your own machine or server.
One browser UI (desktop and mobile) gives you chats with agents, projects and project
groups, a file explorer and editor, a git client, a terminal, named run commands with a
preview proxy, and a system page for the host (Docker, systemd, packages).

Supported agents (the "harness"): Claude Code, Codex (via ACP), OpenCode and Grok. The hub
launches their CLIs as child processes, normalizes their output into one event feed, and
relays permission requests to the UI.

There is no authentication yet: the server binds to `127.0.0.1` by default. Never change
defaults in a way that exposes the hub on a public interface.

## Repository layout

pnpm workspace, TypeScript everywhere, ES modules, Node.js >= 24 (runs `.ts` directly via
type stripping — there is no server build step).

```
apps/server     Hono HTTP + SSE API, agent runtime, SQLite storage
apps/web        React 19 + Vite + Tailwind 4 single-page app
packages/shared Types shared by server and web (@hubness/shared)
crates/         Rust server, API client and terminal UI (Cargo workspace)
```

### Server (`apps/server/src`)

- `index.ts` — app entry: routes, static web build, graceful shutdown. `dev.ts` — dev runner
  that restarts the server on any change in `src`.
- `config.ts` — all `HUBNESS_*` env settings, data/hub directories, `childEnv()` (strips
  hub settings and secrets from child process environments).
- `routes/` — one Hono router per domain (`projects`, `sessions`, `files`, `git`,
  `kanban`, `environment`, `harness`, `system`); `http.ts` holds zod helpers.
- `db/` — `node:sqlite` (`DatabaseSync`) repositories; schema changes go through
  `db/migrations.ts` (append a new migration, never edit an applied one).
- `agents/` — one module per agent (`claude/`, `codex/`, `opencode/`, `grok/`) implementing
  `AgentModule` from `agents/types.ts`, registered in `agents/registry.ts`. The core talks
  to agents only through that interface: adding an agent is a new module plus one line in
  the registry. `acp.ts` is the shared Agent Client Protocol client, `kit.ts` shared helpers.
- `runtime.ts` — session turns, event persistence and broadcast; `bus.ts` — in-process
  pub/sub feeding SSE streams.
- `environments.ts` — where Run/Shell/agents execute: `local` or `sandbox` (bubblewrap).
  `workspaces/` — workspace providers (currently local only).
- `runs.ts`, `terminal.ts`, `preview.ts` — named run commands, interactive shells and the
  per-port preview proxy (`HUBNESS_PREVIEW_PORTS`).
- `git*.ts` — git operations, credentials helper and provider (GitHub/GitLab…) logins.
- `secrets.ts` — encrypted per-project secrets (`HUBNESS_SECRET_KEY` or `<data>/secret.key`).
- `manifest.ts` — project manifest `.hub/hub.yaml` (links, hosts, containers, units),
  shown in the UI and passed to the agent as context.
- `mcp/hub-mcp.ts` — stdio MCP server attached to agent sessions (`open_pane` tool).
- `mcp-servers.ts`, `extensions.ts` — Harness → MCP (servers × agents, written to each agent's
  own config through `agents/mcp-config.ts`) and each agent's skills / rules / subagents /
  commands; every agent module declares its own paths (`agents/<kind>/extensions.ts`).
- `system/` — host monitoring and the "ask about this" flow for containers, units, packages.
- `csrf.ts` — mutating `/api` requests require JSON or the `X-Hubness-Request` header.

### Web (`apps/web/src`)

- `main.tsx` — routes; `pages/` — top-level screens (chats, projects, groups, git,
  harness, system, settings); `components/` — UI building blocks.
- `lib/api.ts` and `lib/query.ts` — API client and TanStack Query hooks; `lib/stream.ts` — SSE.
- `i18n/` — `en.ts` and `ru.ts`. Every user-facing string goes through i18n and must be
  added to both files.
- Vite proxies `/api` to the server in dev; in production the server serves `apps/web/dist`.

### Rust (`crates/`)

The Rust server implements the whole `/api` of the TypeScript server. Both use the same
`HUBNESS_*` settings and the same SQLite file, so they can be switched on one data folder
(not run at the same time). Until the TypeScript server is retired, an API change goes into
both servers, backed by a contract test.

- `core` — API types (`hubness-core`), JSON shape identical to `packages/shared`.
- `server` — axum + rusqlite. `db/migrations.rs` must mirror `db/migrations.ts` exactly:
  a new migration is added to both servers. `README.md` lists the domains and deviations.
  `web.rs` serves the web app: `HUBNESS_WEB_DIST` when set, else the copy compiled in with
  the `embed-web` feature (`build.rs`, off by default, needs `pnpm build`), else `apps/web/dist`.
- `client` — HTTP + SSE client of the API (`hubness-client`).
- `tui` — ratatui terminal UI on top of the client (`hubness-tui`).
- `hubcli` — the hub binary: `hubcli serve`, `hubcli tui`, plus `mcp` and
  `git-credential-helper` that the server starts from its own executable (`current_exe`)
  for agents and git. Feature `embed-web` compiles the web app in.
- `hubtui` — the client-only binary (just the TUI, no server code): `hubtui --url …`.
  TUI flags live in `hubness_tui::cli::Args`, shared by both binaries.
- `install.sh` (repo root) — `./install.sh tui | full | uninstall`; `full` builds the web
  app, installs both binaries and a systemd user service `hubness`.
- `apps/server/test/contract.test.ts` — black-box HTTP tests every server must pass; they
  run against TS in `pnpm test` and against Rust with `pnpm test:contract:rust`.

## Commands

```sh
pnpm install
pnpm dev      # server (:8787, auto-restart) + Vite (:5173) in parallel
pnpm check    # tsc --noEmit for every package (server checks src and test)
pnpm test     # server tests (node:test)
pnpm build    # typecheck and build the web app
pnpm start    # production server, serves the built web app

cargo run -p hubcli -- serve    # the Rust server (same port and data folder)
cargo run -p hubtui             # terminal UI for a running hub (--url, HUBNESS_URL)
pnpm check:rust                 # cargo fmt --check, clippy -D warnings, cargo test
pnpm test:contract:rust         # contract tests against the Rust server
```

Run a single test file: `cd apps/server && node --test test/git.test.ts`.

Always run `pnpm check` and `pnpm test` before considering a change done; for changes in
`crates/` also `pnpm check:rust` and `pnpm test:contract:rust`.

## Conventions

- Code, identifiers, comments, commits and docs are in English.
- Server imports use explicit `.ts` extensions; only erasable TypeScript syntax (no enums,
  no namespaces, no parameter properties) because Node strips types at runtime.
- Validate request bodies with zod in routes; throw `HttpError` for client errors.
- Shared request/response types live in `packages/shared`, not duplicated in apps.
- Child processes get `childEnv()` — never leak `HUBNESS_*` values or secrets to agents.
- Agent output normalizers are covered by fixture tests (`test/fixtures/agents/*.jsonl`,
  `normalizers.test.ts` snapshot); update fixtures when changing an adapter.
- Local data lives in `.data/` (SQLite, logs, keys) and is git-ignored. Do not commit it.

## Git workflow

Git Flow: `main` holds releases, `develop` is the integration branch. Work happens on
`feature/<name>` branches from `develop`, merged back with `--no-ff`. Releases go through
`release/<version>`, hotfixes through `hotfix/<version>`. Commit messages follow
Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `refactor:`).
