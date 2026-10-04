import assert from "node:assert/strict";
import { mergeIntervals } from "./solution.ts";

assert.deepEqual(mergeIntervals([[1, 3], [2, 4]]), [[1, 4]]);
console.log("basic OK");
