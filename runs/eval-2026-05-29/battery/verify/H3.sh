#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# H3 create-file: agent creates schema.sql defining a products table
# (id integer pk / name text not null / price real) plus >=2 INSERTs.
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f schema.sql ] || fail "schema.sql was not created"

out=$(python - <<'PY' 2>&1
import sqlite3, sys
con = sqlite3.connect(":memory:")
try:
    with open("schema.sql", encoding="utf-8") as f:
        con.executescript(f.read())
except sqlite3.Error as e:
    print("ERR: " + type(e).__name__ + ": " + str(e))
    sys.exit(2)

cols = con.execute("PRAGMA table_info(products)").fetchall()
if not cols:
    print("ERR: no 'products' table")
    sys.exit(3)
# PRAGMA table_info -> (cid, name, type, notnull, dflt, pk)
names = {c[1].lower() for c in cols}
need = {"id", "name", "price"}
missing = need - names
if missing:
    print("ERR: products missing columns: " + ",".join(sorted(missing)))
    sys.exit(4)

n = con.execute("SELECT COUNT(*) FROM products").fetchone()[0]
print("ROWS=%d" % n)
print("OK")
PY
)
rc=$?

if [ "$rc" -ne 0 ]; then
  fail "schema.sql invalid or products table not as required: $(echo "$out" | tr '\n' ' ')"
fi

rows=$(echo "$out" | sed -n "s/^ROWS=//p")
if [ "${rows:-0}" -ge 2 ]; then
  pass "products table created with id/name/price and $rows seeded rows"
fi
fail "products table OK but only $rows row(s) inserted (need >= 2)"
