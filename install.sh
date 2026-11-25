#!/bin/sh
# Install hubness: `hubcli` (the hub, with the web app inside) and `hubtui` (terminal UI).
#
#   curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/Tsunami43/hubness/main/install.sh | sh -s -- --tui-only
#
# Options (or environment variables):
#   --tui-only            only hubtui, the client for a hub elsewhere     HUBNESS_TUI_ONLY=1
#   --version X           a release instead of the latest one             HUBNESS_VERSION=X
#   --dir DIR             where the binaries go (default ~/.local/bin)    HUBNESS_INSTALL_DIR=DIR
#   --from-source         build with cargo (hubcli also needs Node.js)    HUBNESS_FROM_SOURCE=1
#   --uninstall           remove the binaries and the service (data stays)
#
# Re-running it upgrades in place. It never starts the hub: see the next steps it prints.
set -eu

REPO="Tsunami43/hubness"
PNPM_VERSION="11.29.0"
# Where releases live: $RELEASES/latest/download/VERSION and $RELEASES/download/v<version>/…
RELEASES="${HUBNESS_RELEASE_URL:-https://github.com/$REPO/releases}"
SOURCE_REPO="${HUBNESS_SOURCE_REPO:-https://github.com/$REPO}"

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
Install hubness: hubcli (the hub, web app inside) and hubtui (terminal UI).

  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- [options]

  --tui-only      only hubtui, the client for a hub elsewhere
  --version X     a release instead of the latest one
  --dir DIR       where the binaries go (default ~/.local/bin)
  --from-source   build with cargo (hubcli also needs Node.js 24+)
  --data-dir DIR  where the hub keeps its data; starts the service without asking
  --no-start      only install; don't offer to start the hub
  --uninstall     remove the binaries and the service (data stays)

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

target_triple() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "no release for the $arch CPU: try --from-source" ;;
  esac
  case "$os" in
    Linux) say "$arch-unknown-linux-gnu" ;;
    Darwin) say "$arch-apple-darwin" ;;
    *) die "no release for $os: try --from-source" ;;
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
  name="hubness-$VERSION-$target"
  url="$RELEASES/download/v$VERSION/$name.tar.gz"
  step "Downloading hubness $VERSION for $target"
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
  if [ "$TUI_ONLY" != 1 ]; then
    have node || die "hubcli embeds the web app, which needs Node.js 24+ to build (or use --tui-only)"
    have npx || die "npx (from Node.js) is needed to run pnpm"
    step "Building the web app (pnpm $PNPM_VERSION via npx)"
    (cd "$TMP/src" && npx -y "pnpm@$PNPM_VERSION" install --frozen-lockfile --silent &&
      npx -y "pnpm@$PNPM_VERSION" --filter @hubness/web run build >/dev/null) || die "the web build failed"
    step "Building hubcli (cargo, release; this takes a few minutes)"
    cargo install --quiet --locked --features embed-web --root "$TMP/root" --path "$TMP/src/crates/hubcli" || die "cargo failed to build hubcli"
  fi
  step "Building hubtui (cargo, release)"
  cargo install --quiet --locked --root "$TMP/root" --path "$TMP/src/crates/hubtui" || die "cargo failed to build hubtui"
}

# ---------------------------------------------------------------- install / uninstall

do_install() {
  if [ "$TUI_ONLY" = 1 ]; then bins="hubtui"; else bins="hubcli hubtui"; fi
  upgraded=0
  if [ "$TUI_ONLY" != 1 ] && [ -x "$INSTALL_DIR/hubcli" ]; then upgraded=1; fi
  if [ "$FROM_SOURCE" = 1 ]; then install_source; else install_release; fi
  mkdir -p "$INSTALL_DIR"
  for b in $bins; do
    [ -f "$BUILT/$b" ] || die "$b is missing from the build"
    # copy then rename: a running hubcli keeps its old file
    cp "$BUILT/$b" "$INSTALL_DIR/.$b.new"
    chmod 755 "$INSTALL_DIR/.$b.new"
    mv -f "$INSTALL_DIR/.$b.new" "$INSTALL_DIR/$b"
    ok "installed $INSTALL_DIR/$b"
  done
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) on_path=1 ;;
    *) on_path=0 ;;
  esac
  say ""
  if [ "$on_path" = 0 ]; then
    warn "$INSTALL_DIR is not on your PATH. Add it to your shell profile:"
    say "    export PATH=\"$INSTALL_DIR:\$PATH\""
    say ""
  fi
  if [ "$TUI_ONLY" = 1 ]; then
    say "${B}Next:${N}"
    say "  hubtui                      ${D}asks for the hub's address on the first run${N}"
  elif [ "$upgraded" = 1 ]; then
    say "${B}Upgraded.${N} If the hub runs as a service, restart it:"
    say "  hubcli server restart"
  elif ! setup_hub; then
    say "${B}Next:${N}"
    say "  hubcli server start         ${D}run the hub in the background (http://127.0.0.1:8787)${N}"
    say "  hubcli user add <name>      ${D}an account: needed to reach the hub from elsewhere${N}"
    say "  hubtui                      ${D}terminal UI (or open the URL in a browser)${N}"
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
  { have systemctl && systemctl --user show-environment >/dev/null 2>&1; } || return 1
  hubcli="$INSTALL_DIR/hubcli"
  if [ -z "$DATA_DIR" ]; then
    has_tty || return 1 # no terminal (CI, provisioning): pass --data-dir to start
    say ""
    ask "Start the hub now as a background service?" "Y/n"
    case "$answer" in [nN]*) return 1 ;; esac
    ask "Where should the hub keep its data (database, logs, keys)?" "~/.local/share/hubness"
    DATA_DIR="$answer"
  fi
  case "$DATA_DIR" in
    "~") DATA_DIR="$HOME" ;;
    "~/"*) DATA_DIR="$HOME/${DATA_DIR#"~/"}" ;;
  esac
  step "Starting the hub (data: $DATA_DIR)"
  "$hubcli" server start --data-dir "$DATA_DIR" </dev/null || return 1
  if has_tty && [ -z "$("$hubcli" user list 2>/dev/null)" ]; then
    ask "Create an account now? Username, empty to skip" ""
    if [ -n "$answer" ]; then
      "$hubcli" user add "$answer" </dev/tty || warn "no account created: run hubcli user add $answer"
    fi
  fi
  say ""
  say "${B}Ready.${N} Open http://127.0.0.1:8787 or run ${B}hubtui${N}."
  say "  hubcli server status | logs -f | config    ${D}manage it later${N}"
  return 0
}

do_uninstall() {
  if [ -x "$INSTALL_DIR/hubcli" ]; then
    "$INSTALL_DIR/hubcli" server uninstall 2>/dev/null || true
  fi
  for b in hubcli hubtui; do
    if [ -e "$INSTALL_DIR/$b" ]; then
      rm -f "$INSTALL_DIR/$b"
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
