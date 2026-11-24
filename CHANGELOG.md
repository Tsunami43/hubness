# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
