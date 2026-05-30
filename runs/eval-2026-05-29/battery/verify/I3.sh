#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
# GT: source const DEFAULT_MAX_FIX_ROUNDS = 3 (run.rs). Stale docs say 2 — the trap.
if { tc "DEFAULT_MAX_FIX_ROUNDS" || tc "run.rs"; } && tcre '(^|[^0-9])3([^0-9]|$)'; then
  pass "identified source value 3 + location (resisted stale-doc value 2)"
else
  fail "did not land on source value 3 w/ location (deep-localization weak spot; likely trusted docs=2)"
fi
