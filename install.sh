#!/usr/bin/env sh
# Claudette one-line installer — Linux & macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh
#
# Env overrides:
#   CLAUDETTE_VERSION         Pin a version (e.g. 0.5.2). Default: latest.
#   CLAUDETTE_INSTALL_DIR     Install location. Default: $HOME/.local/bin.
#   CLAUDETTE_NO_MODIFY_PATH  Set to anything to suppress PATH hints.
#
# What this script does, in order:
#   1. Detects OS + arch and maps to the Rust target triple we ship.
#   2. Resolves the requested tag (latest by default) from the GitHub API.
#   3. Downloads claudette-<tag>-<target>.tar.gz + .sha256 sidecar.
#   4. Verifies the SHA256 (refuses to install on mismatch).
#   5. Drops the `claudette` binary into the install dir.
#   6. Prints a PATH update hint if the install dir isn't on PATH.
#
# We deliberately use POSIX sh, not bash — to keep this script runnable on
# minimal Alpine/macOS shells without surprise dependencies.

set -eu

REPO="mrdushidush/claudette"
INSTALL_DIR="${CLAUDETTE_INSTALL_DIR:-$HOME/.local/bin}"

err()  { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }
info() { printf '\033[1;32m::\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33m!\033[0m %s\n'  "$1"; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"
}

need curl
need tar
need uname
need mktemp

case "$(uname -s)" in
  Linux*)  OS=unknown-linux-gnu ;;
  Darwin*) OS=apple-darwin ;;
  *)       err "unsupported OS: $(uname -s). Try 'cargo install claudette' or download from GitHub Releases manually." ;;
esac

case "$(uname -m)" in
  x86_64|amd64)   ARCH=x86_64 ;;
  aarch64|arm64)  ARCH=aarch64 ;;
  *)              err "unsupported arch: $(uname -m). Try 'cargo install claudette'." ;;
esac

TARGET="${ARCH}-${OS}"

VERSION="${CLAUDETTE_VERSION:-}"
if [ -z "$VERSION" ]; then
  info "resolving latest release tag..."
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$TAG" ] || err "could not resolve latest tag from GitHub API (rate-limited? try CLAUDETTE_VERSION=x.y.z)"
else
  TAG="v${VERSION#v}"
fi

STEM="claudette-${TAG}-${TARGET}"
ARCHIVE="${STEM}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"
SHA_URL="${URL}.sha256"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

info "downloading ${ARCHIVE}"
curl -fsSL "$URL"     -o "${TMP}/${ARCHIVE}" \
  || err "download failed: ${URL} (does this release exist?)"
curl -fsSL "$SHA_URL" -o "${TMP}/${ARCHIVE}.sha256" \
  || err "checksum download failed: ${SHA_URL}"

info "verifying SHA256"
(
  cd "$TMP"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${ARCHIVE}.sha256" >/dev/null
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${ARCHIVE}.sha256" >/dev/null
  else
    err "neither 'shasum' nor 'sha256sum' available — cannot verify download"
  fi
) || err "checksum mismatch — refusing to install"

info "extracting"
tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP"

mkdir -p "$INSTALL_DIR"
mv "${TMP}/claudette" "${INSTALL_DIR}/claudette"
chmod +x "${INSTALL_DIR}/claudette"

info "installed ${TAG} → ${INSTALL_DIR}/claudette"

case ":$PATH:" in
  *:"$INSTALL_DIR":*)
    info "next: claudette --doctor"
    ;;
  *)
    if [ -z "${CLAUDETTE_NO_MODIFY_PATH:-}" ]; then
      printf '\n'
      warn "${INSTALL_DIR} is not on your PATH."
      printf '  Add this to your shell profile (~/.bashrc, ~/.zshrc, ~/.profile):\n\n'
      printf '    export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
      printf '  Then run: claudette --doctor\n'
    fi
    ;;
esac
