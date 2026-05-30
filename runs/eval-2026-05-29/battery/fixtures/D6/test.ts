import { add, sub, mul, isEven, max } from './calc.ts';

let passed = 0;
const total = 6;

function check(label: string, fn: () => boolean): void {
  if (fn()) {
    passed++;
  } else {
    console.error('FAIL: ' + label);
  }
}

check('add', () => add(2, 3) === 5);
check('sub', () => sub(10, 4) === 6);
check('mul', () => mul(6, 7) === 42);
check('isEven true', () => isEven(8) === true);
check('isEven false', () => isEven(7) === false);
check('max', () => max(3, 9) === 9);

console.log(`${passed}/${total} passed`);
if (passed !== total) {
  process.exit(1);
}
