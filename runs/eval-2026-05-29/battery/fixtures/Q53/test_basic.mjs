import assert from "node:assert/strict";
import { parseCsv } from "./solution.mjs";

assert.deepEqual(parseCsv("a,b,c"), [["a", "b", "c"]]);
assert.deepEqual(parseCsv("a,b\nc,d"), [["a", "b"], ["c", "d"]]);
console.log("basic OK");
