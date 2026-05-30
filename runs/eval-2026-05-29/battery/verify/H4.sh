#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# H4 debug-from-error: report.sql had `SUM(o.total)` (no such column);
# fix to `SUM(o.amount)` so it runs and returns each customer's total.
# Expected rows include Ada=150, Bob=50.
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f schema.sql ] || fail "schema.sql missing"
[ -f report.sql ] || fail "report.sql missing"

out=$(python - <<'PY' 2>&1
import sqlite3, sys
con = sqlite3.connect(":memory:")
try:
    with open("schema.sql", encoding="utf-8") as f:
        con.executescript(f.read())
    with open("report.sql", encoding="utf-8") as f:
        query = f.read()
    rows = con.execute(query).fetchall()
except sqlite3.Error as e:
    print("ERR: " + type(e).__name__ + ": " + str(e))
    sys.exit(2)

# Build name -> total mapping from the two returned columns (name, sum).
agg = {}
for r in rows:
    if len(r) < 2:
        print("ERR: row has fewer than 2 columns: %r" % (r,))
        sys.exit(3)
    agg[str(r[0])] = r[1]

ada = agg.get("Ada")
bob = agg.get("Bob")
print("ADA=%r BOB=%r ROWS=%d" % (ada, bob, len(rows)))
if ada is None or float(ada) != 150.0:
    print("ERR: Ada total != 150")
    sys.exit(4)
if bob is None or float(bob) != 50.0:
    print("ERR: Bob total != 50")
    sys.exit(5)
print("OK")
PY
)
rc=$?

if [ "$rc" -ne 0 ]; then
  fail "report.sql still broken or wrong totals: $(echo "$out" | tr '\n' ' ')"
fi
pass "report.sql runs cleanly; Ada=150, Bob=50 ($(echo "$out" | sed -n 's/^ADA=//p'))"
