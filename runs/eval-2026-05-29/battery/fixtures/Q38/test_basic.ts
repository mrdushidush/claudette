import assert from "node:assert/strict";
import { safeJsonParse } from "./solution.ts";

const r = safeJsonParse<{ a: number }>('{"a":1}');
assert.strictEqual(r.ok, true);
if (r.ok) assert.deepEqual(r.value, { a: 1 });
console.log("basic OK");
