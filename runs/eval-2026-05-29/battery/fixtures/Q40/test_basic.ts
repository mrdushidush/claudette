import assert from "node:assert/strict";
import { EventEmitter } from "./solution.ts";

const ee = new EventEmitter();
let count = 0;
ee.on("tick", () => {
  count += 1;
});
ee.emit("tick");
assert.strictEqual(count, 1);
console.log("basic OK");
