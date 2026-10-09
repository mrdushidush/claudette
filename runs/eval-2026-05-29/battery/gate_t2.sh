#!/usr/bin/env bash
# gate_t2.sh — authoring gate for the TIER 2 (unpublished) corpus.
#
# Same test-first discipline as gate_q50.sh, pointed at the T* tasks: a task may
# enter manifest-t2.tsv only if verify/<id>.sh FAILs on the untouched fixture and
# PASSes on fixture + reference solution.
#
# Tier 2 deliberately keeps its own entry point. gate_q50.sh with no arguments
# still globs fixtures/Q* only, so the published corpus and the sealed corpus can
# always be gated independently — and a tier-2 task can never wander into a
# published-corpus run by being picked up in a glob.
#
# usage:  bash gate_t2.sh [id ...]      (no args = every fixtures/T* task)
set -u
BAT="${BATTERY_HOME:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"

ids=("$@")
if [ "${#ids[@]}" -eq 0 ]; then
  mapfile -t ids < <(ls -d "$BAT"/fixtures/T* 2>/dev/null | xargs -n1 basename | sort)
fi

if [ "${#ids[@]}" -eq 0 ]; then
  echo "no tier-2 fixtures found under $BAT/fixtures/T*"
  exit 1
fi

exec bash "$BAT/gate_q50.sh" "${ids[@]}"
