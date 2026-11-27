# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-11-28

### Added

- SSH connections (System → SSH): the hub keeps a key per server — generated or imported,
  encrypted like secrets, never shown again — and attaches connections to projects and
  groups with a note. Agent sessions there use them by name (`ssh <name>`, `scp`, git over
  SSH) through a per-turn ssh-agent, so the agent never sees a key, and read each
  connection's environment and description to tell prod from dev. A connection reaches
  agents only after "Test" has connected once and pinned the server's host key.
- A browser in the Preview tab: the address bar follows navigation inside the app, with
  back, forward and reload; phone, tablet and laptop sizes with rotation; full screen; a QR
  code for a real phone; a console of the page's errors and failed requests; "pick an
  element" that drafts a message for the agent.
- A browser for agents: `preview_open`, `preview_screenshot`, `preview_click`,
  `preview_type`, `preview_press`, `preview_eval` and `preview_console` drive a headless
  Chromium on the hub, so an agent looks at the app it builds — also at a phone size —
  instead of asking. Needs Chromium or Chrome on the hub machine (`HUBNESS_CHROME`).
- Four visual styles, each with its own palettes: Expressive (the default), Glass, Paper
  and Strict.
- A git client in the web app: changes, staging, commits, branches and history as a
  source-control view.
- A chat-first project view on phones, a five-item bottom bar and quick replies.
- Ten more interface languages (13 in all), with search on the language page.
- TUI: a `?` help panel and one key table, fuzzy pickers and live filters, Alt+Tab and
  Alt+Shift+Tab between tabs, the web app's logo as the header, sign-in to GitHub with a
  one-time code, an SSH section and an Account tab.

### Changed

- **One account.** hubness is single-user: the account set at the first sign-in is the only
  one. Roles, member permissions and Settings → Users are gone; Settings → Account changes
  the password and signs out (now also on phones). Agents share the machine's configs, git
  logins and files, so per-user rights could not be enforced. On upgrade the first account
  stays and any other account is deleted with its sessions.
- `hubness user` is `add` (while the hub has no account), `passwd` and `show`; `rm`, `role`
  and `list` are gone. The API has `PUT /api/auth/password` instead of `/api/users`.
- The preview proxy adds a small helper script to previewed pages (console, address bar,
  element picker); `HUBNESS_PREVIEW_INJECT=0` turns it off.
- Touch screens have roomier controls; boxes, tabs and bars are rounded in the smooth styles.
- The README is short; the details moved to `docs/guide.md`.

### Fixed

- The installed web app has PNG icons.
- TUI: `gg`, `G`, Ctrl+d and Ctrl+u work in every list.

## [0.3.8] - 2026-11-27

### Changed

- Voice input settings are simpler: Off / Local / Cloud service with one-line
  descriptions, and Groq and Mistral (Voxtral) as the cloud presets. The request to a
  service no longer sends `response_format`.
- On a phone the sessions panel takes the whole screen, and project groups are a
  full-width list whose project names end with an ellipsis instead of running off the
  screen.

### Fixed

- The focused field stays in view when the phone keyboard opens, also low in a sheet.
- The send button in the chat input is clearly visible on every theme.

## [0.3.7] - 2026-11-27

### Added

- Sign in to GitHub with a one-time code: for a GitHub host the git login form offers
  "Sign in with GitHub" — the hub shows a code and a link to github.com/login/device,
  waits for the approval and saves the token as the login, also with two-factor auth. It
  runs the GitHub CLI (downloaded by the hub when the host has none) in a throwaway config,
  away from the system keyring and the git config.

## [0.3.6] - 2026-11-27

### Fixed

- The voice setup overlay no longer hangs at 100% when the download ends while its live
  stream reconnects: a reconnect that finds the job already finished moves on to choosing
  the model and building whisper.cpp.

## [0.3.5] - 2026-11-27

### Changed

- Voice input setup is one click: choosing any model downloads it (with a percentage),
  chooses it and builds whisper.cpp if the hub has none, all in one overlay that waits for
  the steps; a failed step shows its log.

## [0.3.4] - 2026-11-27

### Added

- The git login form suggests hosts under the host field: the hosts of the projects'
  remotes that have no login yet, then github.com, gitlab.com, codeberg.org and
  bitbucket.org; a click fills the host in and shows its token link.

## [0.3.3] - 2026-11-27

### Added

- Adding a git login links to the token page of the host — GitHub, GitLab (also
  self-hosted), Gitea, Forgejo, Codeberg or Bitbucket — with the repository scopes already
  filled in where the hosting allows it, so a login works with two-factor auth without
  hunting through settings. An unknown domain gets the GitLab and Gitea links; a pasted URL
  or `git@host:repo` remote is reduced to its host.

## [0.3.2] - 2026-11-27

### Changed

- Voice input builds whisper.cpp by itself: choosing a downloaded model starts the build
  when the hub has none, and the page waits behind a progress overlay. The build no longer
  needs cmake or git on the host — it downloads the source archive and, if cmake is missing,
  a portable cmake into the data folder; only a C++ compiler and make are required. Models
  can be chosen only once downloaded.

## [0.3.1] - 2026-11-27

### Added

- User roles: an account is an admin (everything, including accounts) or a member. An admin
  gives a member projects (or all of them), sections (git, files, terminal and Run, board,
  System, Harness, hub settings), agents and models in Settings → Users → Access; the server
  enforces them and filters project, group and session lists. Chats outside projects belong
  to whoever started them. Existing accounts stay admins; the last admin can't be deleted or
  demoted, and `hubness user role <name> admin|member` changes a role from the shell.
- Eleven more interface languages: Ukrainian, German, French, Spanish, Italian, Portuguese,
  Polish, Turkish, Chinese, Japanese and Korean, each loaded only when chosen, and a search
  on the language page.

## [0.3.0] - 2026-11-27

### Added

- Voice input: a microphone in the chat input records speech and puts the transcript into
  the input to check before sending. Settings → Voice input picks where it is transcribed —
  whisper.cpp on the host (found on PATH or built from source by the hub, models from Tiny
  to Large v3 Turbo downloaded from the page, ffmpeg needed) or any OpenAI-compatible
  service such as Groq, OpenAI or speaches (the API key is stored encrypted). Browsers give
  the microphone only on localhost or over HTTPS; over plain HTTP the button offers an audio
  file instead.

## [0.2.4] - 2026-11-27

### Changed

- Mobile web app is easier to use with a finger: buttons, tabs, headers and list rows get
  a 44px hit area on touch screens (the desktop keeps its dense layout), the sessions drawer
  closes with a swipe to the right or a tap beside it, a session is archived by swiping its
  row to the left, and a session inside a project opens full screen like a chat.

## [0.2.3] - 2026-11-26

### Fixed

- Mobile web app feels native: the on-screen keyboard shrinks the screen instead of
  scrolling the page, the bottom navigation hides above it, fields no longer zoom the page
  in on focus (pinch and double-tap zoom are off too), the page doesn't bounce, the chat
  keeps its last message in view, and the sessions drawer and full-screen sheets slide in.

### Changed

- Releases ship two builds: Linux x86_64 and Apple silicon Macs; other machines use
  `install.sh --from-source`. CI checks Ubuntu and macOS.

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
