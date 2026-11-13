#!/bin/sh
# Parqueter installer.
#
#   curl -fsSL https://raw.githubusercontent.com/clarityai-eng/parqueter/master/install.sh | sh
#
# macOS: installs Parqueter.app into ~/Applications (Spotlight, Launchpad, .parquet
#        file association) and links the `parqueter` command into ~/.local/bin.
# Linux: installs the `parqueter` binary into ~/.local/bin.
#
# Environment overrides:
#   PARQUETER_VERSION   release tag to install, e.g. v0.2.0 (default: latest)
#   PARQUETER_BIN_DIR   where to put the `parqueter` command (default: ~/.local/bin)
#   PARQUETER_APP_DIR   macOS only, where to put Parqueter.app (default: ~/Applications)
set -eu

REPO="clarityai-eng/parqueter"
VERSION="${PARQUETER_VERSION:-latest}"
BIN_DIR="${PARQUETER_BIN_DIR:-$HOME/.local/bin}"
APP_DIR="${PARQUETER_APP_DIR:-$HOME/Applications}"

if [ "$VERSION" = "latest" ]; then
  BASE_URL="https://github.com/$REPO/releases/latest/download"
else
  BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
fi

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  arm64 | aarch64) arch="aarch64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) die "unsupported architecture: $arch" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

download() {
  say "Downloading $1"
  curl -fsSL "$BASE_URL/$1" -o "$tmp/$1" || die "download failed: $BASE_URL/$1"
}

mkdir -p "$BIN_DIR"

case "$os" in
  Darwin)
    asset="parqueter-macos-$arch-app.tar.gz"
    download "$asset"
    tar xzf "$tmp/$asset" -C "$tmp"
    [ -d "$tmp/Parqueter.app" ] || die "archive does not contain Parqueter.app"

    mkdir -p "$APP_DIR"
    rm -rf "$APP_DIR/Parqueter.app"
    mv "$tmp/Parqueter.app" "$APP_DIR/Parqueter.app"
    # curl does not set the quarantine flag; clear it anyway in case the archive carried one.
    xattr -dr com.apple.quarantine "$APP_DIR/Parqueter.app" 2>/dev/null || true

    ln -sf "$APP_DIR/Parqueter.app/Contents/MacOS/parqueter" "$BIN_DIR/parqueter"

    # Register with Launch Services so the .parquet association works right away.
    lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    [ -x "$lsregister" ] && "$lsregister" -f "$APP_DIR/Parqueter.app" || true

    say "Installed $APP_DIR/Parqueter.app"
    say "Linked    $BIN_DIR/parqueter"
    ;;
  Linux)
    [ "$arch" = "x86_64" ] || die "no Linux build for $arch yet"
    asset="parqueter-linux-x86_64.tar.gz"
    download "$asset"
    tar xzf "$tmp/$asset" -C "$tmp"
    install -m 755 "$tmp/parqueter" "$BIN_DIR/parqueter"
    say "Installed $BIN_DIR/parqueter"
    ;;
  *)
    die "unsupported OS: $os (on Windows, download the .zip from https://github.com/$REPO/releases)"
    ;;
esac

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    say ""
    say "$BIN_DIR is not in your PATH. Add this to your shell profile:"
    say "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac
