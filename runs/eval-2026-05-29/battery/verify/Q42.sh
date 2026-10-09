#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: ignore case/spaces/punctuation; alphanumerics only; empty
# and single-char count as palindromes.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { isPalindrome } from "./solution.ts";

assert.strictEqual(isPalindrome("A man, a plan, a canal: Panama"), true, "classic phrase");
assert.strictEqual(isPalindrome("Racecar"), true, "case-insensitive");
assert.strictEqual(isPalindrome("was it a car or a cat i saw"), true, "spaces");
assert.strictEqual(isPalindrome("abba"), true, "simple true");
assert.strictEqual(isPalindrome("abc"), false, "simple false");
assert.strictEqual(isPalindrome("12321"), true, "digits");
assert.strictEqual(isPalindrome(""), true, "empty");
assert.strictEqual(isPalindrome("x"), true, "single char");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden isPalindrome tests passed"
