#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K4 (repo_map PHP, bugfix) — isEven() must be true for even n. The bug is the
# inverted test `$n % 2 == 1`; the fix is `% 2 == 0` (or `=== 0`, or the bitwise
# `& 1) == 0`, or `% 2 != 1`). No PHP runtime is used for correctness — verify by
# FILE STATE: a correct even-test is present AND the inverted `% 2 == 1` is gone.
cd "$WORKDIR" || fail "cannot cd into workdir"
ok=0
grep -Eq -e '% ?2 ?\)? ?===? ?0' numbers.php && ok=1     # % 2 == 0 / === 0 / (%2)==0
grep -Eq -e '& ?1 ?\)? ?===? ?0' numbers.php && ok=1     # bitwise  $n & 1) == 0
grep -Eq -e '% ?2 ?\)? ?!==? ?1' numbers.php && ok=1     # % 2 != 1  (also correct)
buggy=0
grep -Eq -e '% ?2 ?\)? ?===? ?1' numbers.php && buggy=1  # inverted test still present
if [ "$ok" -eq 1 ] && [ "$buggy" -eq 0 ]; then
  pass "isEven now tests evenness correctly"
elif [ "$buggy" -eq 1 ]; then
  fail "isEven still uses the inverted test (% 2 == 1)"
else
  fail "isEven not fixed to a correct even-number test"
fi
