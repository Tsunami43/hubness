# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.2] - 2026-11-26

### Fixed

- The hub builds on macOS (`openpty` takes mutable pointers there); CI now builds and tests
  on macOS on every push, so 0.2.2 is the first release with binaries.

## [0.2.1] - 2026-11-26

### Fixed

- Release builds: both macOS targets build on Apple silicon runners (the Intel and
  macOS 14 images are gone), so 0.2.1 is the first release with binaries.
- `install.sh` passes shellcheck (a literal `~` shown to the user and matched on input).
- CI actions moved to their Node 24 majors.

## [0.2.0] - 2026-11-26

### Added

- Rust server implementing the whole API on the same SQLite data folder, with contract
  tests shared with the TypeScript server, and a terminal UI client on top of it.
- One `hubness` command: `serve`, `server start|stop|restart|status|logs|config|uninstall`
  (systemd user service), `user add|passwd|rm|list`, `tui`, `webapp` and `update`
  (self-update from GitHub Releases, `--check` for scripts). It comes in a full build (the
  hub with the web app inside) and a client build (TUI, webapp, update; no server code).
- User accounts with sign-in in the web app, the TUI and the API; a public address is
  refused while the hub has no accounts.
- `curl | sh` installer (`--tui-only` for the client build, `--from-source`, `--data-dir`)
  and release builds for Linux and macOS on x86_64 and aarch64.
- Harness Extensions: MCP servers in the agents' own configs with a catalog from the MCP
  Registry, skills across agents, rules, subagents, commands and plugins.

### Changed

- The pre-release Rust binaries `hubcli` and `hubtui` are replaced by `hubness` (`hubtui`
  is now `hubness tui`). The installer removes the old binaries and moves a service that
  runs `hubcli` over to `hubness`; `hubness server start` also rewrites such a unit.
- Faster feeds, file trees and the preview proxy (less re-rendering, no blocking I/O).

## [0.1.0] - 2026-11-24

### Added

- pnpm monorepo: `apps/server`, `apps/web`, `packages/shared`.
- Hono API server with SSE streams, SQLite storage and migrations, CSRF guard.
- Agent adapters for Claude Code, Codex (ACP), OpenCode and Grok behind one
  `AgentModule` interface; permission approvals; hub MCP server with `open_pane`.
- Projects, project groups, inbox chats, project manifest `.hub/hub.yaml`.
- Git client: status, staging, commits, branches, history, provider logins.
- Local and bubblewrap sandbox environments, encrypted project secrets.
- Run commands, interactive shells and a per-port preview proxy.
- System page: Docker containers, systemd units, packages, "ask about this".
- Harness page to install, update and sign in to agent CLIs.
- React web UI: chats, workspace with explorer, editor and terminal, settings,
  English and Russian localization.
- Server test suite with agent output fixtures.
- README, CLAUDE.md, CONTRIBUTING.md and MIT license.
