# hubness-server (Rust)

Incremental Rust port of `apps/server`. It reads the same `HUBNESS_*` settings and the
same SQLite database, so the two servers can be switched on one data folder. Routes that
are not ported yet answer `404 {"error": "Not found"}`.

```sh
cargo run -p hubness -- serve      # http://127.0.0.1:8787, data in ./.data
pnpm test:contract:rust            # apps/server/test/contract.test.ts against this server
```

## Porting rules

- The TypeScript server is the reference. A route is ported when its contract test passes
  against both servers (`apps/server/test/contract.test.ts`).
- JSON shapes come from `hubness-core`, which mirrors `packages/shared`.
- `src/db/migrations.rs` mirrors `apps/server/src/db/migrations.ts` step by step. A new
  migration is added to both servers with the same version number.
- Child processes must get the equivalent of `childEnv()` (no `HUBNESS_*`, no secrets).

## Status

| Area | Routes | State |
| --- | --- | --- |
| Core | `GET /health`, CSRF guard, `{error}` responses, web app + SPA fallback | done |
| Database | open, WAL, migrations v1–v3 | done |
| Projects | `GET /projects`, `GET /projects/discover`, `POST /projects/import`, `PATCH /projects/:id`, `DELETE /projects/:id` | done (`?files=1` → 501) |
| Settings | `GET /settings`, `PUT /settings` | done |
| Groups | `GET/POST /groups`, `GET/PATCH/DELETE /groups/:id` (local symlink workspace) | done |
| Projects | `GET /projects/:id` (repo info), `new-repo`, `clone`, manifest, `groups/:id/context` | todo |
| Git logins | `/git-credentials` (needs the AES-256-GCM secrets format) | todo |
| Sessions | list, create (no prompt), detail, patch, delete, archive/restore, empty archive, events, pane, stop, `GET /sessions/:id/stream`, `GET /stream` | done |
| Runtime | messages, prompt on create, approvals, stop, title from the first message, run logs, restart recovery | done |
| Sandbox | bubblewrap wrap for project/group agent runs (same hidden paths as TS) | done |
| Sessions | attachments, convert to project, pane file/diff | todo |
| Agents | Claude Code (stream-json, approvals, limits) | done |
| Agents | Codex and OpenCode (ACP), Grok; the hub MCP server (`open_pane`); image attachments | todo |
| Context | system prompt: hub instructions, project, group | done (manifest section todo) |
| Secrets | `/projects/:id/secrets` (same AES-256-GCM format and keys), passing them to agents | done |
| Files | tree, read/write, search, watch | todo |
| Git | status, diff, log, branches, operations | todo |
| Environment | runs, shells, jobs, preview proxy, secrets, sandbox | todo |
| Kanban | cards CRUD, start an agent from a card, card → review after the turn | done |
| Harness | agent install/accounts/models | todo |
| System | containers, units, packages, overview, ask | todo |

Known gaps in ported routes: the background refresh of repository metadata on
`GET /projects` is not ported; sessions of agents other than Claude Code get an error
notice in the feed. Session events are passed through as JSON (`HubEvent` is
not typed in Rust yet); `serde_json` keeps key order and exact floats so the output is
byte-identical to the TS server.
