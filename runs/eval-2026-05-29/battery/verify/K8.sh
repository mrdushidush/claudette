#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K8 (grep_search glob scoping) — API_TIMEOUT_MS is 8000 in the TypeScript source
# of truth (src/settings.ts). JS decoys hold 3000 (config.js) and 5000
# (legacy/config.js). A correctly *.ts-scoped search returns 8000; an unscoped
# search is fooled into a JS value. PASS iff the transcript reports 8000 (naming
# settings.ts is a bonus). FAIL if it never finds 8000 but reports a JS value.
if tc "8000"; then
  if tc "settings.ts"; then
    pass "found API_TIMEOUT_MS=8000 in settings.ts (.ts scope honored)"
  fi
  pass "reported the .ts value API_TIMEOUT_MS=8000"
fi
if tc "3000" || tc "5000"; then
  fail "reported a JavaScript value (3000/5000) and missed 8000 — .ts-only scope ignored"
fi
fail "did not report API_TIMEOUT_MS=8000 from the .ts files"
