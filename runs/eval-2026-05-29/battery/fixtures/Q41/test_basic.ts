import assert from "node:assert/strict";
import { firstDuplicate } from "./solution.ts";

assert.strictEqual(firstDuplicate([1, 2, 3, 2, 1]), 2);
assert.strictEqual(firstDuplicate([1, 2, 3]), null);
console.log("basic OK");
