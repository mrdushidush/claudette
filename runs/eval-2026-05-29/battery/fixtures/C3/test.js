const assert = require('assert');
const { add, sub, mul, isEven } = require('./math.js');

let passed = 0;
const total = 5;

function check(label, fn) {
  fn();
  passed++;
}

check('add', () => assert.strictEqual(add(2, 3), 5));
check('sub', () => assert.strictEqual(sub(10, 4), 6));
check('mul', () => assert.strictEqual(mul(6, 7), 42));
check('isEven true', () => assert.strictEqual(isEven(8), true));
check('isEven false', () => assert.strictEqual(isEven(7), false));

console.log(`${passed}/${total} passed`);
if (passed !== total) {
  process.exit(1);
}
