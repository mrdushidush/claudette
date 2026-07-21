import assert from "node:assert/strict";
import { flatten } from "./solution.mjs";

assert.deepEqual(flatten([1, [2, 3]]), [1, 2, 3]);
console.log("basic OK");
