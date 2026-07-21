import assert from "node:assert/strict";
import { isPalindrome } from "./solution.ts";

assert.strictEqual(isPalindrome("abba"), true);
assert.strictEqual(isPalindrome("abc"), false);
console.log("basic OK");
