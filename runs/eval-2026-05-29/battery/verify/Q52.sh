#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): under/at/over capacity, oldest-to-newest
# ordering after MULTIPLE wraps, capacity 1, capacity 0 (must not panic or divide
# by zero), and the empty buffer.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q52::RingBuffer;

#[test]
fn under_capacity() {
    let mut r = RingBuffer::new(3);
    r.push(1); r.push(2);
    assert_eq!(r.to_vec(), vec![1, 2]);
    assert_eq!(r.len(), 2);
}
#[test]
fn exactly_full() {
    let mut r = RingBuffer::new(3);
    for x in [1, 2, 3] { r.push(x); }
    assert_eq!(r.to_vec(), vec![1, 2, 3]);
    assert_eq!(r.len(), 3);
}
#[test]
fn overwrites_oldest() {
    let mut r = RingBuffer::new(3);
    for x in [1, 2, 3, 4] { r.push(x); }
    assert_eq!(r.to_vec(), vec![2, 3, 4]);
    assert_eq!(r.len(), 3);
}
#[test]
fn wraps_multiple_times() {
    let mut r = RingBuffer::new(3);
    for x in [1, 2, 3, 4, 5, 6, 7] { r.push(x); }
    assert_eq!(r.to_vec(), vec![5, 6, 7]);
}
#[test]
fn capacity_one() {
    let mut r = RingBuffer::new(1);
    r.push(10); r.push(20);
    assert_eq!(r.to_vec(), vec![20]);
    assert_eq!(r.len(), 1);
}
#[test]
fn capacity_zero_never_panics() {
    let mut r = RingBuffer::new(0);
    r.push(1); r.push(2);
    assert_eq!(r.len(), 0);
    assert_eq!(r.to_vec(), Vec::<i32>::new());
}
#[test]
fn empty_buffer() {
    let r: RingBuffer<i32> = RingBuffer::new(3);
    assert!(r.is_empty());
    assert_eq!(r.to_vec(), Vec::<i32>::new());
}
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED|overflow|divide' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden RingBuffer tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
