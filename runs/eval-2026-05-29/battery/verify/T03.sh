#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# TIER 2 — axis: agent mechanics.
# Three things are graded, and only the first is ordinary coding:
#   (a) hours parse and format correctly, and the existing s/m behaviour survives;
#   (b) the change is COHERENT ACROSS THREE FILES — parse.mjs, format.mjs and the
#       SUPPORTED_UNITS list that index.mjs advertises. Editing two of the three
#       leaves the library lying about itself;
#   (c) NO DEPENDENCY IS ADDED. The sandbox is offline, so reaching for an npm
#       package is both a correctness failure and an environment-awareness one.
# (c) is checked before behaviour so the failure reason is unambiguous.

# ---- (c) mechanics: no new dependencies -------------------------------------
if [ ! -f package.json ]; then
  fail "package.json was deleted"
fi
bare=$(grep -REho "from[[:space:]]+['\"][^'\"]+['\"]" src/ 2>/dev/null \
        | sed -E "s/.*['\"]([^'\"]+)['\"].*/\1/" \
        | grep -vE "^(\.|/|node:)" || true)
if [ -n "$bare" ]; then
  fail "added a non-stdlib dependency: $(echo "$bare" | sort -u | tr '\n' ' ')"
fi
deps=$(node -e "try{const j=JSON.parse(require('fs').readFileSync('package.json','utf8'));const n=Object.keys(j.dependencies||{}).concat(Object.keys(j.devDependencies||{}));console.log(n.join(' '))}catch(e){console.log('PARSE-ERROR')}" 2>&1)
if [ "$deps" = "PARSE-ERROR" ]; then
  fail "package.json is no longer valid JSON"
fi
if [ -n "$deps" ]; then
  fail "declared a new dependency in package.json: $deps"
fi

# ---- (a)+(b) behaviour and cross-file coherence ------------------------------
cat > hidden_gate.mjs <<'JS'
import assert from 'node:assert';
import { parseDuration, formatDuration, describe, SUPPORTED_UNITS } from './src/index.mjs';

// hours parse
assert.strictEqual(parseDuration('1h'), 3600, "parseDuration('1h')");
assert.strictEqual(parseDuration('2h30m'), 9000, "parseDuration('2h30m')");
assert.strictEqual(parseDuration('1h30m45s'), 5445, "parseDuration('1h30m45s')");
assert.strictEqual(parseDuration('24h'), 86400, "parseDuration('24h')");

// existing parse behaviour survives
assert.strictEqual(parseDuration('90s'), 90, "parseDuration('90s')");
assert.strictEqual(parseDuration('5m'), 300, "parseDuration('5m')");
assert.strictEqual(parseDuration('1m30s'), 90, "parseDuration('1m30s')");
assert.throws(() => parseDuration('abc'), /invalid duration/, 'invalid input must still throw');

// hours format
assert.strictEqual(formatDuration(3600), '1h', 'formatDuration(3600)');
assert.strictEqual(formatDuration(7200), '2h', 'formatDuration(7200)');
assert.strictEqual(formatDuration(5445), '1h 30m 45s', 'formatDuration(5445)');
assert.strictEqual(formatDuration(3660), '1h 1m', 'formatDuration(3660)');
assert.strictEqual(formatDuration(3601), '1h 1s', 'formatDuration(3601)');

// existing format behaviour survives
assert.strictEqual(formatDuration(0), '0s', 'formatDuration(0)');
assert.strictEqual(formatDuration(45), '45s', 'formatDuration(45)');
assert.strictEqual(formatDuration(90), '1m 30s', 'formatDuration(90)');
assert.strictEqual(formatDuration(300), '5m', 'formatDuration(300)');

// round trip through the third file
assert.strictEqual(describe('1h30m45s'), '1h 30m 45s', "describe('1h30m45s')");
assert.strictEqual(describe('90s'), '1m 30s', "describe('90s')");

// the library must not lie about what it supports
assert.ok(Array.isArray(SUPPORTED_UNITS), 'SUPPORTED_UNITS must still be an array');
assert.ok(SUPPORTED_UNITS.includes('h'), 'SUPPORTED_UNITS must advertise hours');
assert.ok(SUPPORTED_UNITS.includes('m'), 'SUPPORTED_UNITS must still advertise minutes');
assert.ok(SUPPORTED_UNITS.includes('s'), 'SUPPORTED_UNITS must still advertise seconds');

console.log('ok');
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|message|expected' | head -3 | tr '\n' ' ')"
fi
echo "$out" | grep -q '^ok$' \
  && pass "hours supported across all three files, no dependency added" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
