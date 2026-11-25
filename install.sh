#!/usr/bin/env bash
# Install hubness from this repository.
#
#   ./install.sh tui         the terminal client only (hubtui); needs Rust
#   ./install.sh full        the whole hub (hubcli with the web app inside, plus hubtui)
#                            and a systemd user service; needs Rust, Node.js and pnpm (or npx)
#   ./install.sh uninstall   stop the service and remove the binaries (data stays)
#
# Re-running an install upgrades in place. Run `./install.sh --help` for the options.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PNPM_VERSION="10.13.1"
MIN_RUST="1.85"
SERVICE="hubness"

# Locations; every one can be overridden from the environment (also handy for tests).
BIN_DIR="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin"
CONFIG_HOME="${HUBNESS_INSTALL_CONFIG_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}}"
DATA_HOME="${HUBNESS_INSTALL_DATA_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}}"
CONFIG_DIR="$CONFIG_HOME/hubness"
ENV_FILE="$CONFIG_DIR/hub.env"
UNIT_DIR="$CONFIG_HOME/systemd/user"
UNIT_FILE="$UNIT_DIR/$SERVICE.service"
DEFAULT_DATA_DIR="$DATA_HOME/hubness"

COMMAND=""
HOST=""
PORT=""
SERVICE_ON=1
DRY_RUN=0
PURGE=0
ASSUME_YES=0

# ---------------------------------------------------------------- output

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD=$'\033[1m' DIM=$'\033[2m' RED=$'\033[31m' GREEN=$'\033[32m' YELLOW=$'\033[33m' BLUE=$'\033[34m' RESET=$'\033[0m'
else
  BOLD="" DIM="" RED="" GREEN="" YELLOW="" BLUE="" RESET=""
fi

step() { printf '\n%s==>%s %s%s%s\n' "$BLUE" "$RESET" "$BOLD" "$*" "$RESET"; }
info() { printf '    %s\n' "$*"; }
ok() { printf '    %s✓%s %s\n' "$GREEN" "$RESET" "$*"; }
warn() { printf '    %s!%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die() {
  printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2
  exit 1
}

# Run a command, or only print it with --dry-run.
run() {
  local shown="$*"
  if [ "$1" = pnpm_cmd ]; then shown="$PNPM_LABEL ${*:2}"; fi
  if [ "$DRY_RUN" = 1 ]; then
    printf '    %s[dry-run]%s %s\n' "$DIM" "$RESET" "$shown"
  else
    printf '    %s$ %s%s\n' "$DIM" "$shown" "$RESET"
    "$@"
  fi
}

# Write stdin to a file (creating its folder), or only show it with --dry-run.
write_file() {
  local path="$1" mode="${2:-644}" content
  content="$(cat)"
  if [ "$DRY_RUN" = 1 ]; then
    printf '    %s[dry-run]%s write %s (mode %s):\n' "$DIM" "$RESET" "$path" "$mode"
    printf '%s\n' "$content" | sed "s/^/      ${DIM}|${RESET} /"
    return
  fi
  mkdir -p "$(dirname "$path")"
  printf '%s\n' "$content" >"$path"
  chmod "$mode" "$path"
  ok "wrote $path"
}

# Ask a yes/no question; the default answer is used without a terminal, with --yes it is yes.
confirm() {
  local question="$1" default="${2:-y}" answer hint="[Y/n]"
  [ "$default" = n ] && hint="[y/N]"
  if [ "$ASSUME_YES" = 1 ]; then return 0; fi
  if [ "$DRY_RUN" = 1 ]; then
    printf '    %s[dry-run]%s ask: %s (assuming yes)\n' "$DIM" "$RESET" "$question"
    return 0
  fi
  if [ ! -t 0 ]; then
    [ "$default" = y ]
    return
  fi
  read -r -p "    $question $hint " answer || answer=""
  answer="${answer:-$default}"
  case "$answer" in [Yy]*) return 0 ;; *) return 1 ;; esac
}

usage() {
  cat <<EOF
${BOLD}hubness installer${RESET}

Usage: ./install.sh <command> [options]

Commands:
  tui          Install only the terminal client ${BOLD}hubtui${RESET} (needs Rust).
               Connect it to any hub: hubtui --url http://host:8787
  full         Build the web app and install ${BOLD}hubcli${RESET} (the hub, web app embedded)
               and ${BOLD}hubtui${RESET}; set up and start the systemd user service '$SERVICE'.
  uninstall    Stop and remove the service and the binaries. Data and settings stay
               unless --purge is given.

Options:
  --host HOST    Address the hub listens on (full; default 127.0.0.1). Anything but a
                 loopback address makes the hub reachable from other machines.
  --port PORT    Port of the hub (full; default 8787).
  --no-service   Do not install or start the systemd user service (full).
  --purge        Also delete settings and the hub's data (uninstall).
  --dry-run      Print every action without changing anything.
  -y, --yes      Answer yes to every question.
  -h, --help     Show this help.

Locations (override with the environment):
  binaries       $BIN_DIR   (CARGO_INSTALL_ROOT)
  settings       $ENV_FILE   (HUBNESS_INSTALL_CONFIG_HOME, XDG_CONFIG_HOME)
  service unit   $UNIT_FILE
  data           $DEFAULT_DATA_DIR   (HUBNESS_INSTALL_DATA_HOME, XDG_DATA_HOME;
                 HUBNESS_DATA_DIR in hub.env)

Re-running an install upgrades in place and keeps hub.env.
EOF
}

# ---------------------------------------------------------------- arguments

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      tui | full | uninstall)
        [ -z "$COMMAND" ] || die "only one command at a time (got '$COMMAND' and '$1')"
        COMMAND="$1"
        ;;
      --host)
        [ $# -ge 2 ] || die "--host needs a value"
        HOST="$2"
        shift
        ;;
      --host=*) HOST="${1#*=}" ;;
      --port)
        [ $# -ge 2 ] || die "--port needs a value"
        PORT="$2"
        shift
        ;;
      --port=*) PORT="${1#*=}" ;;
      --no-service) SERVICE_ON=0 ;;
      --purge) PURGE=1 ;;
      --dry-run) DRY_RUN=1 ;;
      -y | --yes) ASSUME_YES=1 ;;
      -h | --help | help)
        usage
        exit 0
        ;;
      *) die "unknown argument '$1' (see ./install.sh --help)" ;;
    esac
    shift
  done
  [ -n "$COMMAND" ] || {
    usage
    exit 1
  }
  if [ -n "$PORT" ]; then
    case "$PORT" in
      '' | *[!0-9]*) die "--port must be a number, got '$PORT'" ;;
    esac
    if [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then die "--port must be between 1 and 65535"; fi
  fi
  if [ -n "$HOST" ] && [ "$COMMAND" != full ]; then warn "--host only applies to 'full'"; fi
  if [ -n "$PORT" ] && [ "$COMMAND" != full ]; then warn "--port only applies to 'full'"; fi
  if [ "$PURGE" = 1 ] && [ "$COMMAND" != uninstall ]; then die "--purge only applies to 'uninstall'"; fi
}

# ---------------------------------------------------------------- checks

have() { command -v "$1" >/dev/null 2>&1; }

version_ge() { # version_ge 1.90.0 1.85 → true
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]
}

check_rust() {
  if ! have cargo && [ -x "$HOME/.cargo/bin/cargo" ]; then PATH="$HOME/.cargo/bin:$PATH"; fi
  have cargo || die "Rust is not installed. Get it from https://rustup.rs and run this again."
  local v
  v="$(cargo --version | awk '{print $2}')"
  version_ge "$v" "$MIN_RUST" || die "Rust $v is too old: $MIN_RUST or newer is needed (rustup update)."
  ok "cargo $v"
}

PNPM_VIA_NPX=0
PNPM_LABEL="pnpm"
pnpm_cmd() {
  if [ "$PNPM_VIA_NPX" = 1 ]; then npx -y "pnpm@$PNPM_VERSION" "$@"; else pnpm "$@"; fi
}

check_node() {
  have node || die "Node.js is not installed: it is needed to build the web app (Node.js 24 or newer)."
  local v
  v="$(node --version | sed 's/^v//')"
  version_ge "$v" "24" || warn "Node.js $v found; hubness is developed with Node.js 24 or newer."
  ok "node $v"
  if have pnpm && pnpm --version >/dev/null 2>&1; then
    ok "pnpm $(pnpm --version)"
  elif have npx; then
    PNPM_VIA_NPX=1
    PNPM_LABEL="npx -y pnpm@$PNPM_VERSION"
    ok "pnpm not found: using npx -y pnpm@$PNPM_VERSION"
  else
    die "neither pnpm nor npx found: install pnpm (https://pnpm.io/installation)."
  fi
}

# Something listening on the port? (ss, else a bash /dev/tcp probe)
port_busy() {
  local port="$1"
  if have ss; then
    ss -Hltn "sport = :$port" 2>/dev/null | grep -q .
  else
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null
  fi
}

is_loopback() {
  case "$1" in
    localhost | 127.* | ::1 | '[::1]') return 0 ;;
    *) return 1 ;;
  esac
}

systemd_user_available() {
  have systemctl && systemctl --user show-environment >/dev/null 2>&1
}

service_active() {
  have systemctl && systemctl --user is-active --quiet "$SERVICE" 2>/dev/null
}

# The URL to reach a hub listening on host:port from this machine.
hub_url() {
  local host="$1" port="$2"
  case "$host" in
    0.0.0.0 | :: | '[::]') host="127.0.0.1" ;;
    \[*) ;;
    *:*) host="[$host]" ;;
  esac
  printf 'http://%s:%s' "$host" "$port"
}

# A value from hub.env (KEY=VALUE or KEY="VALUE"), empty when absent.
env_value() {
  [ -f "$ENV_FILE" ] || return 0
  sed -n "s/^[[:space:]]*$1=//p" "$ENV_FILE" | tail -n1 | sed 's/^"\(.*\)"$/\1/'
}

# ---------------------------------------------------------------- steps

install_crate() { # install_crate <crate dir> [cargo args…]
  local crate="$1"
  shift
  run cargo install --path "$REPO/crates/$crate" --locked --root "${BIN_DIR%/bin}" "$@"
}

warn_path() {
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not in PATH: add it to your shell profile, e.g. export PATH=\"$BIN_DIR:\$PATH\"" ;;
  esac
}

warn_old_binary() {
  if [ -x "$BIN_DIR/hubness" ]; then
    warn "$BIN_DIR/hubness is the old name of hubcli. Stop whatever runs it before using hubcli on the"
    warn "same port and data folder, then remove it: cargo uninstall hubness"
  fi
}

build_web() {
  step "Building the web app"
  run pnpm_cmd --dir "$REPO" install --frozen-lockfile
  run pnpm_cmd --dir "$REPO" --filter @hubness/web run build
  if [ "$DRY_RUN" = 0 ] && [ ! -f "$REPO/apps/web/dist/index.html" ]; then
    die "the web build produced no apps/web/dist/index.html"
  fi
}

# hub.env: written with defaults the first time; later runs only apply --host/--port.
write_env() {
  step "Settings: $ENV_FILE"
  if [ -f "$ENV_FILE" ]; then
    ok "keeping the existing hub.env"
    local key value
    for key in HUBNESS_HOST HUBNESS_PORT; do
      value="$HOST"
      [ "$key" = HUBNESS_PORT ] && value="$PORT"
      [ -n "$value" ] || continue
      [ "$(env_value "$key")" = "$value" ] && continue
      if grep -q "^[[:space:]]*$key=" "$ENV_FILE"; then
        run sed -i "s|^[[:space:]]*$key=.*|$key=$value|" "$ENV_FILE"
      elif [ "$DRY_RUN" = 1 ]; then
        printf '    %s[dry-run]%s append %s=%s to %s\n' "$DIM" "$RESET" "$key" "$value" "$ENV_FILE"
      else
        printf '%s=%s\n' "$key" "$value" >>"$ENV_FILE"
      fi
      ok "set $key=$value"
    done
    return
  fi
  write_file "$ENV_FILE" 600 <<EOF
# hubness settings (read by the '$SERVICE' systemd user service). Restart after a change:
#   systemctl --user restart $SERVICE
# Every setting is described in .env.example in the hubness repository.

# Listen on this machine only. A non-loopback address makes the hub reachable from other
# machines: set up authentication first and put it behind HTTPS or a VPN.
HUBNESS_HOST=${HOST:-127.0.0.1}
HUBNESS_PORT=${PORT:-8787}

# Database, logs and keys.
HUBNESS_DATA_DIR="$DEFAULT_DATA_DIR"

# Hub working folder: inbox, projects, home (default: ~/hub).
# HUBNESS_HOME="$HOME/hub"

# Agents and run commands see this PATH (captured at install time, so the hub finds the
# same agent CLIs as your shell).
PATH="$PATH"
EOF
}

write_unit() {
  step "systemd user service: $UNIT_FILE"
  local env_file="$ENV_FILE" data_dir="$DEFAULT_DATA_DIR"
  # use systemd's %h for paths under the home folder, as the unit is per user anyway
  case "$env_file" in "$HOME"/*) env_file="%h/${env_file#"$HOME"/}" ;; esac
  case "$data_dir" in "$HOME"/*) data_dir="%h/${data_dir#"$HOME"/}" ;; esac
  write_file "$UNIT_FILE" 644 <<EOF
[Unit]
Description=hubness: self-hosted hub for coding agents
Documentation=file://$REPO/README.md

[Service]
Type=simple
# hub.env overrides these defaults
Environment=HUBNESS_DATA_DIR=$data_dir
EnvironmentFile=-$env_file
ExecStart=$BIN_DIR/hubcli serve
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF
}

start_service() {
  step "Starting the service"
  run systemctl --user daemon-reload
  run systemctl --user enable "$SERVICE"
  # restart, not start: a re-run must pick up the new binary
  run systemctl --user restart "$SERVICE"
  if have loginctl && [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null || true)" = no ]; then
    info "The hub stops when you log out. To keep it running (e.g. on a server):"
    info "  sudo loginctl enable-linger $USER"
  fi
}

wait_health() {
  local url="$1" i
  step "Waiting for $url/api/health"
  if [ "$DRY_RUN" = 1 ]; then
    printf '    %s[dry-run]%s poll %s/api/health\n' "$DIM" "$RESET" "$url"
    return 0
  fi
  for i in $(seq 1 30); do
    if have curl; then
      curl -fsS --max-time 2 "$url/api/health" >/dev/null 2>&1 && break
    elif have wget; then
      wget -q -T 2 -O /dev/null "$url/api/health" 2>/dev/null && break
    else
      warn "neither curl nor wget found: cannot check the hub"
      return 0
    fi
    [ "$i" = 30 ] && {
      warn "the hub did not answer within 30 s; see: journalctl --user -u $SERVICE -e"
      return 1
    }
    sleep 1
  done
  ok "the hub is up"
}

# hubcli gets a `user` subcommand with authentication; offer the first user when it exists.
offer_first_user() {
  local hubcli="$BIN_DIR/hubcli" name answer
  if [ "$DRY_RUN" = 1 ]; then
    printf '    %s[dry-run]%s offer: %s user add <name> (when hubcli has the user command)\n' "$DIM" "$RESET" "$hubcli"
    return 0
  fi
  if [ ! -x "$hubcli" ] || ! "$hubcli" user --help >/dev/null 2>&1; then return 0; fi
  step "First user"
  if [ ! -t 0 ] && [ "$ASSUME_YES" = 0 ]; then
    info "Create one later: $hubcli user add <name>"
    return 0
  fi
  confirm "Create a user now?" y || {
    info "Create one later: $hubcli user add <name>"
    return 0
  }
  name="${USER:-admin}"
  if [ -t 0 ]; then
    read -r -p "    User name [$name]: " answer || answer=""
    name="${answer:-$name}"
  fi
  # the same settings (data folder) as the service
  (
    set -a
    # shellcheck disable=SC1090
    [ -f "$ENV_FILE" ] && . "$ENV_FILE"
    set +a
    "$hubcli" user add "$name"
  ) || warn "creating the user failed; try again: $hubcli user add $name"
}

# ---------------------------------------------------------------- commands

cmd_tui() {
  step "Checking requirements"
  check_rust
  step "Installing hubtui"
  install_crate hubtui
  warn_path
  printf '\n%sDone.%s Connect to a hub:\n' "$GREEN$BOLD" "$RESET"
  info "hubtui                              # a hub on this machine (http://127.0.0.1:8787)"
  info "hubtui --url http://host:8787       # a hub on another machine (or HUBNESS_URL)"
  info "For a hub that listens on localhost only, forward its port first:"
  info "  ssh -N -L 8787:127.0.0.1:8787 user@host   # then: hubtui"
}

cmd_full() {
  local host port url
  host="${HOST:-$(env_value HUBNESS_HOST)}"
  host="${host:-127.0.0.1}"
  port="${PORT:-$(env_value HUBNESS_PORT)}"
  port="${port:-8787}"

  step "Checking requirements"
  check_rust
  check_node
  if ! is_loopback "$host"; then
    warn "HUBNESS_HOST=$host makes the hub reachable from other machines, and agents can run any"
    warn "command on this host. Only do this with authentication set up, behind HTTPS or a VPN."
  fi
  if [ "$SERVICE_ON" = 1 ] && ! systemd_user_available; then
    if [ "$DRY_RUN" = 1 ]; then
      warn "no systemd user session here; a real run would need --no-service"
    else
      die "no systemd user session (systemctl --user) here: re-run with --no-service and start 'hubcli serve' yourself"
    fi
  fi
  if port_busy "$port"; then
    if [ "$SERVICE_ON" = 1 ] && service_active; then
      ok "port $port is used by the running '$SERVICE' service (it will be restarted)"
    else
      warn "port $port is already in use: the hub will not start there. Pick another one with --port."
      confirm "Continue anyway?" n || die "aborted: port $port is in use (--port picks another one, --yes continues anyway)"
    fi
  else
    ok "port $port is free"
  fi
  warn_old_binary

  build_web
  step "Installing hubcli (with the web app) and hubtui"
  install_crate hubcli --features embed-web
  install_crate hubtui
  warn_path

  write_env

  url="$(hub_url "$host" "$port")"

  if [ "$SERVICE_ON" = 1 ]; then
    write_unit
    start_service
    wait_health "$url" && offer_first_user
    printf '\n%sDone.%s The hub runs at %s%s%s\n' "$GREEN$BOLD" "$RESET" "$BOLD" "$url" "$RESET"
    info "status:   systemctl --user status $SERVICE"
    info "logs:     journalctl --user -u $SERVICE -f"
    info "settings: $ENV_FILE (then: systemctl --user restart $SERVICE)"
  else
    offer_first_user
    printf '\n%sDone.%s Start the hub yourself:\n' "$GREEN$BOLD" "$RESET"
    info "set -a; . \"$ENV_FILE\"; set +a; \"$BIN_DIR/hubcli\" serve"
    info "then open $url"
  fi
  info "terminal: hubtui --url $url"
}

cmd_uninstall() {
  local data_dir
  data_dir="$(env_value HUBNESS_DATA_DIR)"
  data_dir="${data_dir:-$DEFAULT_DATA_DIR}"

  step "Removing the service"
  # only a unit this installer wrote (in UNIT_DIR) is touched
  if [ -f "$UNIT_FILE" ]; then
    if have systemctl; then run systemctl --user disable --now "$SERVICE" || true; fi
    run rm -f "$UNIT_FILE"
    if have systemctl; then run systemctl --user daemon-reload || true; fi
  else
    ok "no '$SERVICE' service in $UNIT_DIR"
  fi

  step "Removing the binaries from $BIN_DIR"
  local bin
  for bin in hubcli hubtui; do
    if [ ! -e "$BIN_DIR/$bin" ]; then
      ok "$bin is not installed"
    elif have cargo; then
      run cargo uninstall --root "${BIN_DIR%/bin}" "$bin" || run rm -f "$BIN_DIR/$bin"
    else
      run rm -f "$BIN_DIR/$bin"
    fi
  done

  if [ "$PURGE" = 1 ]; then
    step "Deleting settings and data"
    warn "this deletes $CONFIG_DIR and $data_dir (database, logs, keys) for good"
    if confirm "Delete them?" n; then
      run rm -rf "$CONFIG_DIR" "$data_dir"
    else
      info "kept"
    fi
  else
    info "Kept settings ($CONFIG_DIR) and data ($data_dir); --purge deletes them."
  fi
  printf '\n%sDone.%s\n' "$GREEN$BOLD" "$RESET"
}

main() {
  parse_args "$@"
  if [ "$DRY_RUN" = 1 ]; then printf '%sDry run: nothing is changed.%s\n' "$YELLOW" "$RESET"; fi
  case "$COMMAND" in
    tui) cmd_tui ;;
    full) cmd_full ;;
    uninstall) cmd_uninstall ;;
  esac
}

main "$@"
