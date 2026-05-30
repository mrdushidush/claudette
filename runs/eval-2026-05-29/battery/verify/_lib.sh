#!/usr/bin/env bash
# Shared verifier helpers. Source this from each verify/<id>.sh.
# Contract: verify/<id>.sh <WORKDIR> <TRANSCRIPT>; print exactly one
# "RESULT: PASS — ..." or "RESULT: FAIL — ..." line; always exit 0.
WORKDIR="${1:?workdir}"; TRANSCRIPT="${2:?transcript}"
pass(){ echo "RESULT: PASS — ${1:-ok}"; exit 0; }
fail(){ echo "RESULT: FAIL — ${1:-no reason}"; exit 0; }
# transcript contains token (case-insensitive, literal)
tc(){ grep -iqF -- "$1" "$TRANSCRIPT"; }
# transcript matches regex (case-insensitive)
tcre(){ grep -iqE -- "$1" "$TRANSCRIPT"; }
# count of ground-truth tokens (one per arg) present in transcript
tcount(){ local n=0; for t in "$@"; do grep -iqF -- "$t" "$TRANSCRIPT" && n=$((n+1)); done; echo "$n"; }
