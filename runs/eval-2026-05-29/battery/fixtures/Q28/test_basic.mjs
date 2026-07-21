import assert from "node:assert/strict";
import { chunk } from "./solution.mjs";

assert.deepEqual(chunk([1, 2, 3, 4], 2), [[1, 2], [3, 4]]);
console.log("basic OK");
