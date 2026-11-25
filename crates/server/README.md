# hubness-server (Rust)

Rust implementation of the hubness server, a port of `apps/server` covering its whole
`/api`. It reads the same `HUBNESS_*` settings and the same SQLite database, so the two
servers can be switched on one data folder.

```sh
cargo run -p hubness -- serve      # http://127.0.0.1:8787, data in ./.data
cargo run -p hubness -- mcp        # the hub MCP server (agents get it automatically)
pnpm test:contract:rust            # apps/server/test/contract.test.ts against this server
```

## Remote access

Sign-in uses user accounts (username + password, argon2id; all accounts have the same
rights). Without accounts the hub listens on 127.0.0.1 and asks for nothing; the web app
offers to create the first account, which is accepted only from the hub's own machine.
To reach the hub from another machine:

```sh
hubcli user add alice          # password from a hidden prompt (or stdin when not a TTY)
HUBNESS_HOST=0.0.0.0 hubcli serve
```

The hub refuses a non-loopback `HUBNESS_HOST` while it has no accounts. Once an account
exists every `/api` request and every preview port needs a session: `POST /api/auth/login`
`{username, password}` returns `{token, user}` for API clients (`Authorization: Bearer`;
the TUI asks on start and keeps the token per hub URL in `~/.config/hubness/tui.json`,
mode 0600) and sets an HttpOnly `SameSite=Lax` cookie for browsers. Failed sign-ins back
off per username and address. Accounts are managed in Settings → Users (web and TUI),
through `/api/users`, or with `hubcli user add|passwd|rm|list` on the data folder
(`HUBNESS_DATA_DIR`, also while the hub runs). A password change or a deleted account ends
its sessions; the last account can't be deleted. The hub's own children (the MCP server)
call it back with a random per-process service token from their environment.

The hub speaks plain HTTP — across untrusted networks put it behind an HTTPS reverse proxy
(Caddy, nginx) or a VPN (Tailscale, WireGuard). The TypeScript server has no sign-in and
stays loopback-only (it only shares the `users` / `auth_sessions` tables through migration
v4).

## Rules while both servers exist

- An API change goes into both servers, with a contract test that passes against both
  (`apps/server/test/contract.test.ts`).
- JSON shapes come from `hubness-core`, which mirrors `packages/shared`.
- `src/db/migrations.rs` mirrors `apps/server/src/db/migrations.ts` step by step. A new
  migration is added to both servers with the same version number.
- Child processes must get the equivalent of `childEnv()` (no `HUBNESS_*`, no secrets).

## Status

| Area | Routes | State |
| --- | --- | --- |
| Core | `GET /health`, CSRF guard, `{error}` responses, web app + SPA fallback | done |
| Auth | accounts: `/auth/status`, `/auth/login`, `/auth/setup`, `/auth/logout`, `GET/POST /users`, `DELETE /users/:id`, `PUT /users/:id/password`; `hubcli user …` — Rust only | done |
| Database | open, WAL, migrations v1–v4 | done |
| Projects | `GET /projects`, `GET /projects/discover`, `POST /projects/import`, `PATCH /projects/:id`, `DELETE /projects/:id` | done (incl. `?files=1&confirm=` with the TS safety checks) |
| Settings | `GET /settings`, `PUT /settings` | done |
| Groups | `GET/POST /groups`, `GET/PATCH/DELETE /groups/:id` (local symlink workspace) | done |
| Manifest | `GET/POST /projects/:id/manifest`, manifest section of the agent context | done |
| Projects | `GET /projects/:id` (repo info, provider links, languages), `POST /projects/new-repo`, `GET /groups/:id/context`, background repo-meta refresh on `GET /projects` | done |
| Projects | `POST /projects/clone` (a terminal job with the stored host login; the project appears after a successful clone) | done |
| Git logins | `GET /git-credentials`, `PUT/DELETE /git-credentials/:host` (same encrypted rows as TS), credential helper `hubness git-credential-helper` for fetch/pull/push | done |
| Sessions | list, create (no prompt), detail, patch, delete, archive/restore, empty archive, events, pane, stop, `GET /sessions/:id/stream`, `GET /stream` | done |
| Runtime | messages, prompt on create, approvals, stop, title from the first message, run logs, restart recovery | done |
| Sandbox | bubblewrap wrap for project/group agent runs (same hidden paths as TS) | done |
| Sessions | attachments (upload, serve, images to the model), project suggestion, Inbox → project conversion | done |
| Sessions | pane file and diff (`/sessions/:id/file`, `/sessions/:id/diff`) | done |
| Agents | Claude Code (stream-json, approvals, limits) | done |
| Agents | Codex, OpenCode and Grok over ACP (load/resume, models, modes, client-side policy, provider retries) | done |
| MCP | `hubness mcp`: the hub MCP server (`open_pane`, `close_pane`) attached to every agent session | done |
| Context | system prompt: hub instructions, project, group, manifest | done |
| Secrets | `/projects/:id/secrets` (same AES-256-GCM format and keys), passing them to agents | done |
| Files | tree, read/write (conflict-safe), create/rename/delete, file list, browse, search (ripgrep + fallback), `/watch` SSE (inotify) | done |
| Git | `/projects/:id/git/*`: status, diff, branches, log, show, stashes, every `POST /git/:op` operation | done |
| Environment | run commands (`/projects/:id/runs…`: CRUD, suggestions, start/stop, port detection), project and group shells, `GET/PUT /projects/:id/environment`, `…/environment/container/remove` (local, sandbox, docker) | done |
| Jobs | terminal jobs on a pty (`openpty`), `GET /jobs/:id/stream`, `POST /jobs/:id/input`, `POST /jobs/:id/kill` | done |
| Preview | `POST /projects/:id/preview`, per-port proxy (`HUBNESS_PREVIEW_PORTS`, HTTP + upgrades) | done |
| Kanban | cards CRUD, start an agent from a card, card → review after the turn | done |
| Harness | `/agents`, `/agents/:kind/models`, `/harness`, `/harness/agents`, `/harness/:kind` (versions, latest checks, auth, limits, models), default model, provider account ops | done |
| Harness | `POST /harness/:kind/actions/:action` (only actions from the harness's own list, as terminal jobs) | done |
| System | `/system/*`: platform, overview (CPU, memory, disks, GPUs, sensors), Docker containers/networks/volumes (logs, actions, safe removal), systemd units (launchd on macOS), package managers and pacman package info; `POST /ask` | done |

Session events are passed through as JSON (`HubEvent` is
not typed in Rust yet); `serde_json` keeps key order and exact floats so the output is
byte-identical to the TS server.

System commands run without a shell, with `childEnv()`-style environment (TS passes the
full hub environment) and with `--` before user-supplied names where the TS server passes
them unchecked (container actions/logs/inspect, `systemctl --user`, `pacman -Qi`).

Repository languages use GitHub Linguist data generated from the `linguist-languages`
npm package into `src/languages_data.rs`; regenerate it with
`node crates/server/build/languages.mjs` after updating the package.
