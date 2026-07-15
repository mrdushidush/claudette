#!/usr/bin/env bash
# Package one release-binary flavor into dist/ with a SHA256 sidecar.
# Called twice per build-binaries matrix leg (release.yml): once for the lean
# coding-only binary, once for the `--features integrations` full binary.
#
# Env contract (set by the calling step):
#   FLAVOR  "lean" or "full" — picks the archive stem prefix
#           (claudette-… vs claudette-full-…). The binary *inside* the archive
#           is named `claudette` either way, so install.sh / install.ps1
#           extract both flavors identically.
#   TARGET  Rust target triple (matrix.target); locates the built binary.
#   EXT     "tar.gz" or "zip" (matrix.ext).
# Emits `stem=<archive stem>` to $GITHUB_OUTPUT.
#
# Runs under bash on every runner. Windows runners ship `tar` and `7z` but
# only `sha256sum` (no `shasum`); macOS ships `shasum` (perl-based) but no
# `sha256sum`; Linux ships both. Try `shasum` first, fall back to `sha256sum`
# — output format ("<hash>  <filename>") is identical between the two.
set -euo pipefail

# On tag push: GITHUB_REF=refs/tags/v0.5.4 -> version=0.5.4.
# On workflow_dispatch: no tag, so use a `dev-<short-sha>` stamp so the
# archive name stays slash-free and the dry-run is distinguishable from a
# real release.
if [ "${GITHUB_REF#refs/tags/v}" != "${GITHUB_REF}" ]; then
  version="${GITHUB_REF#refs/tags/v}"
else
  version="dev-${GITHUB_SHA:0:7}"
fi

case "${FLAVOR}" in
  lean) prefix="claudette" ;;
  full) prefix="claudette-full" ;;
  *)
    echo "::error::unknown FLAVOR '${FLAVOR}' (expected 'lean' or 'full')"
    exit 1
    ;;
esac

stem="${prefix}-v${version}-${TARGET}"
# Per-flavor staging dir: the two flavors are packaged from the same
# target/<target>/release/claudette path (the full build overwrites the lean
# binary), so each package step must copy its binary aside before the next
# build runs — and must not see the other flavor's copy.
staging="staging-${FLAVOR}"
mkdir -p dist "${staging}"

if [ "${EXT}" = "zip" ]; then
  cp "target/${TARGET}/release/claudette.exe" "${staging}/"
  (cd "${staging}" && 7z a "../dist/${stem}.zip" claudette.exe >/dev/null)
else
  cp "target/${TARGET}/release/claudette" "${staging}/"
  chmod +x "${staging}/claudette"
  tar -C "${staging}" -czf "dist/${stem}.tar.gz" claudette
fi

archive="${stem}.${EXT}"
cd dist
if command -v shasum >/dev/null 2>&1; then
  shasum -a 256 "${archive}" > "${archive}.sha256"
else
  sha256sum "${archive}" > "${archive}.sha256"
fi

echo "stem=${stem}" >> "${GITHUB_OUTPUT}"
