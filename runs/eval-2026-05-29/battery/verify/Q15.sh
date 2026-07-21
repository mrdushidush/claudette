#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: return + call-once on success; retry-then-succeed with
# the right call count; and on total failure re-raise the LAST exception (not None).
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pytest
from solution import retry

def test_returns_on_first_success():
    calls = {"n": 0}
    def f():
        calls["n"] += 1
        return 42
    assert retry(f, 3) == 42
    assert calls["n"] == 1

def test_retries_then_succeeds():
    calls = {"n": 0}
    def f():
        calls["n"] += 1
        if calls["n"] < 3:
            raise RuntimeError("not yet")
        return "ok"
    assert retry(f, 5) == "ok"
    assert calls["n"] == 3

def test_reraises_last_exception():
    calls = {"n": 0}
    def f():
        calls["n"] += 1
        raise ValueError(f"boom {calls['n']}")
    with pytest.raises(ValueError) as excinfo:
        retry(f, 3)
    assert "boom 3" in str(excinfo.value)
    assert calls["n"] == 3
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed|raise' | head -4 | tr '\n' ' ')"
fi
pass "all hidden retry tests passed"
