#!/bin/sh
# Install hubness: the `hubness` command (the hub with the web app inside, its management
# commands and the terminal UI), or with --tui-only its client build (no server code).
#
#   curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --tui-only
#
# Options (or environment variables):
#   --tui-only            the client build, for a hub elsewhere           HUBNESS_TUI_ONLY=1
#   --version X           a release instead of the latest one             HUBNESS_VERSION=X
#   --dir DIR             where the binary goes (default ~/.local/bin)    HUBNESS_INSTALL_DIR=DIR
#   --from-source         build with cargo (the full build needs Node.js) HUBNESS_FROM_SOURCE=1
#   --data-dir DIR        start the hub service with this data folder
#   --no-start            only install
#   --uninstall           remove the binary and the service (data stays)
#
# Re-running it upgrades in place (so does `hubness update`). Pre-0.2 binaries (`hubcli`,
# `hubtui`) in the same folder are replaced, and a service running hubcli moves to hubness.
set -eu

REPO="Tsunami43/hubness"
PNPM_VERSION="11.29.0"
# Where releases live: $RELEASES/latest/download/VERSION and $RELEASES/download/v<version>/…
RELEASES="${HUBNESS_RELEASE_URL:-https://github.com/$REPO/releases}"
SOURCE_REPO="${HUBNESS_SOURCE_REPO:-https://github.com/$REPO}"
# tests swap systemctl for a fake (the hubness binary honors the same variable)
SYSTEMCTL="${HUBNESS_SYSTEMCTL:-systemctl}"

INSTALL_DIR="${HUBNESS_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${HUBNESS_VERSION:-}"
TUI_ONLY="${HUBNESS_TUI_ONLY:-0}"
FROM_SOURCE="${HUBNESS_FROM_SOURCE:-0}"
DATA_DIR=""
START=auto
ACTION=install

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  B=$(printf '\033[1m') D=$(printf '\033[2m') R=$(printf '\033[31m') G=$(printf '\033[32m') Y=$(printf '\033[33m') N=$(printf '\033[0m')
else
  B="" D="" R="" G="" Y="" N=""
fi

say() { printf '%s\n' "$*"; }
step() { printf '%s›%s %s\n' "$B" "$N" "$*"; }
ok() { printf '%s✓%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%s!%s %s\n' "$Y" "$N" "$*" >&2; }
die() {
  printf '%serror:%s %s\n' "$R" "$N" "$*" >&2
  exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<USAGE
Install hubness: the hubness command (hub with the web app inside, terminal UI, updates).

  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- [options]

  --tui-only      the client build (terminal UI, webapp) for a hub elsewhere
  --version X     a release instead of the latest one
  --dir DIR       where the binary goes (default ~/.local/bin)
  --from-source   build with cargo (the full build also needs Node.js 24+)
  --data-dir DIR  where the hub keeps its data; starts the service without asking
  --no-start      only install; don't offer to start the hub
  --uninstall     remove the binary and the service (data stays)

Environment: HUBNESS_VERSION, HUBNESS_INSTALL_DIR, HUBNESS_TUI_ONLY=1, HUBNESS_FROM_SOURCE=1
(with HUBNESS_SOURCE_REF=<branch> to build another branch).
USAGE
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --tui-only) TUI_ONLY=1 ;;
      --from-source) FROM_SOURCE=1 ;;
      --uninstall) ACTION=uninstall ;;
      --version)
        [ $# -ge 2 ] || die "--version needs a value, e.g. --version 0.2.0"
        VERSION="$2"
        shift
        ;;
      --version=*) VERSION="${1#*=}" ;;
      --dir)
        [ $# -ge 2 ] || die "--dir needs a folder"
        INSTALL_DIR="$2"
        shift
        ;;
      --dir=*) INSTALL_DIR="${1#*=}" ;;
      --data-dir)
        [ $# -ge 2 ] || die "--data-dir needs a folder"
        DATA_DIR="$2"
        START=yes
        shift
        ;;
      --data-dir=*)
        DATA_DIR="${1#*=}"
        START=yes
        ;;
      --no-start) START=no ;;
      -h | --help)
        usage
        exit 0
        ;;
      *) die "unknown option: $1 (see --help)" ;;
    esac
    shift
  done
  VERSION="${VERSION#v}"
  if [ "$TUI_ONLY" = 1 ]; then FLAVOR=client; else FLAVOR=full; fi
}

# ---------------------------------------------------------------- downloads

# fetch URL FILE: download to a file (curl, else wget).
fetch() {
  if have curl; then
    curl -fsSL --retry 2 -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    die "neither curl nor wget found"
  fi
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "no sha256sum or shasum to verify the download"
  fi
}

# Releases are built for Linux x86_64 and Apple silicon Macs; others build from source.
target_triple() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os/$arch" in
    Linux/x86_64 | Linux/amd64) say "x86_64-unknown-linux-gnu" ;;
    Darwin/arm64 | Darwin/aarch64) say "aarch64-apple-darwin" ;;
    *) die "no ready-made build for $os on $arch (releases cover Linux x86_64 and Apple silicon Macs).
  Build it here instead: curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --from-source" ;;
  esac
}

install_release() {
  target=$(target_triple)
  if [ -z "$VERSION" ]; then
    step "Looking for the latest release"
    fetch "$RELEASES/latest/download/VERSION" "$TMP/VERSION" 2>/dev/null ||
      die "no hubness release is published yet. Build it from source instead:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --from-source"
    VERSION=$(tr -d ' \r\n' <"$TMP/VERSION")
    VERSION="${VERSION#v}"
    [ -n "$VERSION" ] || die "the release has an empty VERSION file"
  fi
  name="hubness-$VERSION-$target-$FLAVOR"
  url="$RELEASES/download/v$VERSION/$name.tar.gz"
  step "Downloading hubness $VERSION for $target ($FLAVOR build)"
  fetch "$url" "$TMP/$name.tar.gz" || die "can't download $url
  (is $VERSION a released version? Or try --from-source)"
  fetch "$url.sha256" "$TMP/$name.tar.gz.sha256" || die "can't download the checksum $url.sha256"
  want=$(cut -d' ' -f1 <"$TMP/$name.tar.gz.sha256")
  got=$(sha256_of "$TMP/$name.tar.gz")
  [ "$want" = "$got" ] || die "checksum mismatch for $name.tar.gz (expected $want, got $got)"
  ok "checksum verified"
  tar -xzf "$TMP/$name.tar.gz" -C "$TMP"
  BUILT="$TMP/$name"
}

install_source() {
  have cargo || die "--from-source needs Rust (cargo): https://rustup.rs"
  have git || die "--from-source needs git"
  ref="${HUBNESS_SOURCE_REF:-main}"
  [ -z "$VERSION" ] || ref="v$VERSION"
  step "Fetching the source ($SOURCE_REPO, $ref)"
  git clone -q --depth 1 --branch "$ref" "$SOURCE_REPO" "$TMP/src" || die "can't clone $SOURCE_REPO at $ref"
  BUILT="$TMP/root/bin"
  if [ "$FLAVOR" = full ]; then
    have node || die "the full build embeds the web app, which needs Node.js 24+ to build (or use --tui-only)"
    have npx || die "npx (from Node.js) is needed to run pnpm"
    step "Building the web app (pnpm $PNPM_VERSION via npx)"
    (cd "$TMP/src" && npx -y "pnpm@$PNPM_VERSION" install --frozen-lockfile --silent &&
      npx -y "pnpm@$PNPM_VERSION" --filter @hubness/web run build >/dev/null) || die "the web build failed"
    features="--features embed-web"
  else
    features="--no-default-features"
  fi
  step "Building hubness ($FLAVOR, cargo release; this takes a few minutes)"
  # shellcheck disable=SC2086 # $features is two words on purpose
  cargo install --quiet --locked $features --root "$TMP/root" --path "$TMP/src/crates/hubness" || die "cargo failed to build hubness"
}

# ---------------------------------------------------------------- install / uninstall

do_install() {
  upgraded=0
  if [ "$FLAVOR" = full ] && [ -x "$INSTALL_DIR/hubness" ]; then upgraded=1; fi
  check_old
  if [ "$FROM_SOURCE" = 1 ]; then install_source; else install_release; fi
  mkdir -p "$INSTALL_DIR"
  [ -f "$BUILT/hubness" ] || die "hubness is missing from the build"
  # copy then rename: a running hub keeps its old file
  cp "$BUILT/hubness" "$INSTALL_DIR/.hubness.new"
  chmod 755 "$INSTALL_DIR/.hubness.new"
  mv -f "$INSTALL_DIR/.hubness.new" "$INSTALL_DIR/hubness"
  ok "installed $INSTALL_DIR/hubness ($("$INSTALL_DIR/hubness" --version | head -n 1))"
  migrate_old
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) on_path=1 ;;
    *) on_path=0 ;;
  esac
  say ""
  if [ "$on_path" = 0 ]; then
    warn "$INSTALL_DIR is not on your PATH. Add it to your shell profile:"
    say "    export PATH=\"$INSTALL_DIR:\$PATH\""
    say ""
  else
    found=$(command -v hubness 2>/dev/null || true)
    if [ -n "$found" ] && [ "$found" != "$INSTALL_DIR/hubness" ]; then
      warn "another hubness comes first on your PATH: $found (remove it, or put $INSTALL_DIR before it)"
      say ""
    fi
  fi
  if [ "$FLAVOR" = client ]; then
    say "${B}Next:${N}"
    say "  hubness tui                 ${D}asks for the hub's address on the first run${N}"
    say "  hubness webapp --url ADDR   ${D}open the hub's web app in the browser${N}"
  elif [ "$MIGRATED" = 1 ]; then
    :
  elif [ "$upgraded" = 1 ]; then
    if service_active; then
      step "Restarting the hub service on the new binary"
      "$INSTALL_DIR/hubness" server restart </dev/null || warn "the restart failed: run hubness server restart"
    else
      say "${B}Upgraded.${N} Later upgrades: ${B}hubness update${N}"
    fi
  elif ! setup_hub; then
    say "${B}Next:${N}"
    say "  hubness server start        ${D}run the hub in the background (http://127.0.0.1:8787)${N}"
    say "  hubness user add <name>     ${D}an account: needed to reach the hub from elsewhere${N}"
    say "  hubness tui                 ${D}terminal UI (or: hubness webapp)${N}"
  fi
}

unit_file() { say "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/hubness.service"; }

have_systemd() { have "$SYSTEMCTL" && "$SYSTEMCTL" --user show-environment >/dev/null 2>&1; }

service_active() { have_systemd && [ "$("$SYSTEMCTL" --user is-active hubness.service 2>/dev/null)" = active ]; }

# Before installing: is there a pre-0.2 install (hubcli, hubtui) whose service runs hubcli?
check_old() {
  OLD_SERVICE=0
  OLD_ACTIVE=0
  MIGRATED=0
  if [ -f "$(unit_file)" ] && grep -q 'hubcli' "$(unit_file)"; then
    OLD_SERVICE=1
    if service_active; then OLD_ACTIVE=1; fi
  fi
}

# After installing: remove hubcli/hubtui and move a hubcli service over to hubness.
migrate_old() {
  for b in hubcli hubtui; do
    [ -e "$INSTALL_DIR/$b" ] || continue
    if [ "$b" = hubcli ] && [ "$OLD_SERVICE" = 1 ] && [ "$FLAVOR" = client ]; then
      warn "kept $INSTALL_DIR/hubcli: the hub service still runs it and the client build has no server"
      continue
    fi
    rm -f "${INSTALL_DIR:?}/${b:?}"
    ok "removed the old $INSTALL_DIR/$b (hubness replaces it)"
  done
  [ "$OLD_SERVICE" = 1 ] && [ "$FLAVOR" = full ] || return 0
  MIGRATED=1
  if [ "$OLD_ACTIVE" = 1 ]; then
    step "Moving the running hub service from hubcli to hubness"
    if "$INSTALL_DIR/hubness" server start </dev/null; then
      ok "the service now runs $INSTALL_DIR/hubness"
    else
      warn "the service didn't start: run hubness server start"
    fi
  else
    say ""
    say "The hub service still points at hubcli; switch it to hubness (it starts it):"
    say "  hubness server start"
  fi
}

# A terminal to ask on: under `curl | sh` stdin is the script itself.
has_tty() { [ -r /dev/tty ] && (: </dev/tty) 2>/dev/null; }

# ask QUESTION DEFAULT: read an answer from the terminal into $answer.
ask() {
  printf '%s %s[%s]%s ' "$1" "$D" "$2" "$N" >/dev/tty
  read -r answer </dev/tty || answer=""
  [ -n "$answer" ] || answer="$2"
}

# First install: start the hub as a service (asking where its data goes) and offer an
# account. Returns 1 when it didn't start the hub (the caller prints the next steps).
setup_hub() {
  [ "$START" != no ] || return 1
  have_systemd || return 1
  hub="$INSTALL_DIR/hubness"
  if [ -z "$DATA_DIR" ]; then
    has_tty || return 1 # no terminal (CI, provisioning): pass --data-dir to start
    say ""
    ask "Start the hub now as a background service?" "Y/n"
    case "$answer" in [nN]*) return 1 ;; esac
    # shellcheck disable=SC2088 # shown to the user as typed; expanded just below
    ask "Where should the hub keep its data (database, logs, keys)?" "~/.local/share/hubness"
    DATA_DIR="$answer"
  fi
  # shellcheck disable=SC2088 # matching a literal ~ the user typed
  case "$DATA_DIR" in
    "~") DATA_DIR="$HOME" ;;
    "~/"*) DATA_DIR="$HOME/${DATA_DIR#"~/"}" ;;
  esac
  step "Starting the hub (data: $DATA_DIR)"
  "$hub" server start --data-dir "$DATA_DIR" </dev/null || return 1
  if has_tty && [ -z "$("$hub" user list 2>/dev/null)" ]; then
    ask "Create an account now? Username, empty to skip" ""
    if [ -n "$answer" ]; then
      "$hub" user add "$answer" </dev/tty || warn "no account created: run hubness user add $answer"
    fi
  fi
  say ""
  say "${B}Ready.${N} Open $("$hub" webapp --print) (${B}hubness webapp${N}) or run ${B}hubness tui${N}."
  say "  hubness server status | logs -f | config   ${D}manage it later${N}"
  say "  hubness update                             ${D}upgrade to a new release${N}"
  return 0
}

do_uninstall() {
  # the full build (or a pre-0.2 hubcli) removes the service; the client build has none
  for b in hubness hubcli; do
    if [ -x "$INSTALL_DIR/$b" ] && "$INSTALL_DIR/$b" server uninstall </dev/null 2>/dev/null; then break; fi
  done
  for b in hubness hubcli hubtui; do
    if [ -e "$INSTALL_DIR/$b" ]; then
      rm -f "${INSTALL_DIR:?}/${b:?}"
      ok "removed $INSTALL_DIR/$b"
    fi
  done
  say "Your data and settings stay (~/.local/share/hubness, ~/.config/hubness); delete them by hand if you want."
}

main() {
  parse_args "$@"
  if [ "$ACTION" = uninstall ]; then
    do_uninstall
    return
  fi
  TMP=$(mktemp -d 2>/dev/null || mktemp -d -t hubness)
  trap 'rm -rf "$TMP"' EXIT
  trap 'exit 130' INT TERM
  do_install
}

main "$@"
