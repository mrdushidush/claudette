#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer test: 8 threads each incrementing 20_000 times must total
# exactly 160_000. The read-modify-write-across-two-locks bug loses updates.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q11::Counter;
use std::sync::Arc;
use std::thread;

#[test]
fn concurrent_increments_are_not_lost() {
    let counter = Arc::new(Counter::new());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            for _ in 0..20_000 {
                c.increment();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(counter.get(), 8 * 20_000);
}

#[test]
fn single_threaded_counts() {
    let counter = Counter::new();
    for _ in 0..100 {
        counter.increment();
    }
    assert_eq!(counter.get(), 100);
}
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|left|right|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "concurrent increments not lost" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
