import assert from "node:assert/strict";
import { loadConfig } from "./config.mjs";

// With no user overrides, the defaults come through.
assert.strictEqual(loadConfig({}).server.host, "localhost");
console.log("basic OK");
