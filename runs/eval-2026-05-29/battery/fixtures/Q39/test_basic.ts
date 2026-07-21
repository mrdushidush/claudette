import assert from "node:assert/strict";
import { makeGrid } from "./solution.ts";

assert.deepEqual(makeGrid(2, 3), [
  [0, 0, 0],
  [0, 0, 0],
]);
console.log("basic OK");
