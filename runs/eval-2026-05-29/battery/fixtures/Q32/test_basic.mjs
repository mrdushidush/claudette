import assert from "node:assert/strict";
import { intersection } from "./solution.mjs";

assert.deepEqual(intersection([1, 2, 3, 4], [2, 4, 6]), [2, 4]);
console.log("basic OK");
