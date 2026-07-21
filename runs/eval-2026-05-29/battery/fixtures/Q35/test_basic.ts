import assert from "node:assert/strict";
import { parseQueryString } from "./solution.ts";

assert.deepEqual(parseQueryString("a=1&b=2"), { a: "1", b: "2" });
console.log("basic OK");
