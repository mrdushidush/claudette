import assert from 'node:assert';
import { parseDuration, formatDuration } from './src/index.mjs';

assert.strictEqual(parseDuration('90s'), 90);
assert.strictEqual(parseDuration('5m'), 300);
assert.strictEqual(formatDuration(90), '1m 30s');
console.log('ok');
