import assert from "node:assert/strict";
import { deepClone } from "./solution.ts";

const original = { a: 1 };
const copy = deepClone(original);
assert.deepEqual(copy, { a: 1 });
assert.notStrictEqual(copy, original);
console.log("basic OK");
