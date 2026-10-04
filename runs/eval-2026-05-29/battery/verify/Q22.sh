#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer test: 8 threads x 5000 increments must total exactly 40000.
# The unlocked read-modify-write loses updates under contention.
cat > hidden_gate_test.py <<'PY'
import os, sys, threading
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import Counter

def test_concurrent_increments_not_lost():
    c = Counter()
    def worker():
        for _ in range(5000):
            c.increment()
    threads = [threading.Thread(target=worker) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert c.get() == 8 * 5000

def test_single_thread():
    c = Counter()
    for _ in range(100):
        c.increment()
    assert c.get() == 100
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "concurrent increments not lost"
