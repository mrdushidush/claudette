#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# D2 — locate symbol: refreshToken is defined ONLY in src/auth/session.ts.
# PASS iff the transcript names session.ts. Fail-guard: if it names ONLY a decoy
# file (index/user/util) without session.ts, that is a wrong answer.
tc "session.ts" || {
  if tc "index.ts" || tc "user.ts" || tc "util.ts"; then
    fail "named a decoy file but not session.ts (refreshToken lives in src/auth/session.ts)"
  fi
  fail "transcript does not identify session.ts as the definition site"
}

# Bonus signal (not required to pass): mentioning the auth/ path.
if tcre "auth[/\\]session\.ts"; then
  pass "located refreshToken in src/auth/session.ts (full path given)"
fi
pass "located refreshToken in session.ts"
