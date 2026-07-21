import assert from "node:assert/strict";
import { range } from "./solution.ts";

assert.deepEqual(range(0, 5), [0, 1, 2, 3, 4]);
assert.deepEqual(range(0, 10, 2), [0, 2, 4, 6, 8]);
console.log("basic OK");
