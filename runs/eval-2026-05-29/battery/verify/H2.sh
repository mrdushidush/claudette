#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# H2 add-feature: agent writes top_customer.sql returning the name of the
# customer with the highest total order amount. Expected single row: 'Ada'.
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f schema.sql ]       || fail "schema.sql missing"
[ -f top_customer.sql ] || fail "top_customer.sql missing"

out=$(python - <<'PY' 2>&1
import sqlite3, sys
con = sqlite3.connect(":memory:")
try:
    with open("schema.sql", encoding="utf-8") as f:
        con.executescript(f.read())
    with open("top_customer.sql", encoding="utf-8") as f:
        query = f.read()
    if not any(kw in query.upper() for kw in ("SELECT", "WITH")):
        print("ERR: top_customer.sql contains no query")
        sys.exit(3)
    rows = con.execute(query).fetchall()
except sqlite3.Error as e:
    print("ERR: " + type(e).__name__ + ": " + str(e))
    sys.exit(2)
if not rows:
    print("ERR: query returned no rows")
    sys.exit(4)
print("NROWS=%d" % len(rows))
print("FIRST=%r" % (rows[0][0],))
PY
)
rc=$?

if [ "$rc" -ne 0 ]; then
  fail "top_customer.sql did not produce a usable result: $(echo "$out" | tr '\n' ' ')"
fi

first=$(echo "$out" | sed -n "s/^FIRST=//p")
nrows=$(echo "$out" | sed -n "s/^NROWS=//p")

# Accept 'Ada' (with single or double quotes from repr).
if echo "$first" | grep -qiE "^'?\"?Ada\"?'?$"; then
  if [ "$nrows" = "1" ]; then
    pass "top_customer.sql returns exactly one row: Ada"
  fi
  pass "top_customer.sql returns Ada as the top customer (rows=$nrows)"
fi
fail "top_customer.sql first cell was $first (expected Ada); rows=$nrows"
