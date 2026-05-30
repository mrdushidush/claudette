#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# F4 — refactor/rename: process_file -> handle_file everywhere.
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f lib.sh ] || fail "lib.sh missing in workdir"
[ -f run.sh ] || fail "run.sh missing in workdir"

# Prefer ripgrep when present, else POSIX grep -r (verifiers run via
# non-interactive `bash`, where rg may be off PATH).
if command -v rg >/dev/null 2>&1; then
  search(){ rg -n --no-heading "$1" .; }
else
  search(){ grep -rn -- "$1" .; }
fi

# Old name must be entirely gone (definition + both call sites).
old="$(search 'process_file')"
if [ -n "$old" ]; then
  fail "old name 'process_file' still present:\n$old"
fi

# New name must appear at least 3 times (1 definition + 2 calls).
hits="$(search 'handle_file' | grep -c . )"
[ "${hits:-0}" -ge 3 ] || fail "expected >=3 'handle_file' occurrences (defn + 2 calls), found ${hits:-0}"

pass "process_file fully renamed to handle_file ($hits occurrences)"
