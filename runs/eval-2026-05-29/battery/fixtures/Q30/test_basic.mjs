import assert from "node:assert/strict";
import { sortNumbers } from "./solution.mjs";

assert.deepEqual(sortNumbers([3, 1, 2]), [1, 2, 3]);
console.log("basic OK");
