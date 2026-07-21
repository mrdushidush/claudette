import assert from "node:assert/strict";
import { safeGet } from "./solution.mjs";

assert.strictEqual(safeGet({ a: { b: 1 } }, "a.b"), 1);
console.log("basic OK");
