# hubness guide

Installing, running and configuring a hub in detail. The short version is in the
[README](../README.md).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh
```

It downloads the latest release for your OS and CPU, checks its sha256 and puts one
command, `hubness`, into `~/.local/bin`: the hub (server with the web app inside), its
management commands, the terminal UI and self-update. `--tui-only` installs the client build
of the same command instead — the terminal UI and `webapp` for a hub on another machine, no
server code.

On a first install in a terminal it asks where the hub keeps its data (default
`~/.local/share/hubness`), starts it as a service and offers to create an account. Later the
folder is shown in Settings → Storage (web) and Settings → Connection (TUI) and changed with
`hubness server config data-dir <path>` + `hubness server restart`.

Options go after `sh -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --tui-only     # client build: TUI only
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --version 0.2.0
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --from-source  # cargo (+ Node.js for the web app)
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --data-dir /srv/hubness  # no questions
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --no-start     # binaries only
curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --uninstall    # data stays
```

`--dir DIR` (or `HUBNESS_INSTALL_DIR`) installs elsewhere. Upgrade with `hubness update`
(`--check` only reports; it restarts the service on the new binary), or run the installer
again. Coming from a pre-0.2 build (`hubcli`, `hubtui`)? The installer removes them and
moves the service over to `hubness`.

## Usage

**Run the hub** as a systemd user service:

```sh
hubness server start          # http://127.0.0.1:8787, starts again at login (asks for the data folder the first time)
hubness user add alice        # an account: sign-in for the web app, the TUI and the API
hubness server status         # running? health, URL, data folder, account
hubness server logs -f        # its log
```

Open the URL in a browser (`hubness webapp`), or run `hubness tui`. On a server, `sudo loginctl enable-linger $USER`
keeps the hub running after you log out and across reboots (`server start` reminds you).
Without systemd (macOS, containers) run `hubness serve` under your own process manager.

**Settings** live in `~/.config/hubness/hub.env`; data (database, logs, keys) in
`~/.local/share/hubness`:

```sh
hubness server config                        # show host, port, data-dir
hubness server config port 9000              # then: hubness server restart
hubness server start --data-dir ~/hubness/.data   # keep using an existing data folder
```

**The terminal UI.** The first `hubness tui` asks for the hub's address — a domain or an IP,
with `:port` when it isn't 8787 (a bare host tries `https://` first, then `http://…:8787`;
Enter alone is this machine). It signs in if the hub has its account and remembers the hub, so
the next start opens it directly. `Space H` switches between saved hubs or adds one;
`hubness tui --url <address>` opens another one from the command line.

**Remote use.** The hub listens on `127.0.0.1` by default. The simplest way in keeps it
there and forwards the port over SSH:

```sh
ssh -N -L 8787:127.0.0.1:8787 you@server    # on your laptop, then: hubness tui (address 127.0.0.1)
```

To listen on the network, create an account first and set the host — hubness refuses a
public address while there is no account:

```sh
hubness user add alice
hubness server config host 0.0.0.0 && hubness server restart
```

Plain HTTP is fine on a VPN (Tailscale, WireGuard); across the internet put the hub behind an
HTTPS reverse proxy and connect with `hubness tui --url hub.example.com`. See
[Remote access](../crates/server/README.md#remote-access).

Browsers allow the microphone (voice input) only on `localhost` or over HTTPS: on a phone
use an SSH tunnel, `tailscale serve --https=443 http://127.0.0.1:8787` or the HTTPS proxy.
Over plain HTTP the microphone button offers an audio file instead.

## Agents and their tools

- **SSH connections** (System → SSH): the hub keeps a key per server; add its public key on
  the server, press Test, then attach the connection to a project or group. Agent sessions
  there use it as `ssh <name>` without ever seeing the key. Needs OpenSSH on the hub machine.
- **Browser for agents**: agents open the app they build in a headless Chromium on the hub
  (`preview_*` tools) to look at screenshots, click and read the console. Needs Chromium or
  Chrome on the hub machine (`HUBNESS_CHROME` points at another binary).

## Configuration

All settings are environment variables (see [`.env.example`](../.env.example)):

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
| `HUBNESS_CHROME` | first Chromium on `PATH` | Browser for the agents' preview tools |
| `HUBNESS_PREVIEW_INJECT` | on | `0` stops the preview proxy from adding its helper script to pages |

> **Security.** hubness gives agents shell access to the host. Keep it on `127.0.0.1`
> unless it has its account (`hubness user add`; the TypeScript dev server has none), and put
> it behind HTTPS or a VPN before it leaves your machine.
