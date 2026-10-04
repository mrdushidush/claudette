#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: user overrides scalar defaults; nested merge keeps sibling
# keys; defaults still present; DEFAULTS not mutated between calls.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { loadConfig } from "./config.mjs";

assert.strictEqual(loadConfig({ debug: true }).debug, true, "user overrides scalar");

const cfg = loadConfig({ server: { port: 9090 } });
assert.strictEqual(cfg.server.port, 9090, "nested override");
assert.strictEqual(cfg.server.host, "localhost", "nested sibling kept");

assert.strictEqual(loadConfig({}).server.port, 8080, "defaults present");
assert.strictEqual(loadConfig({ extra: 1 }).extra, 1, "user-only key added");

// Overriding once must not leak into the next call (DEFAULTS not mutated).
loadConfig({ debug: true });
assert.strictEqual(loadConfig({}).debug, false, "defaults not mutated");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|nested|mutated' | head -4 | tr '\n' ' ')"
fi
pass "all hidden config-merge tests passed"
