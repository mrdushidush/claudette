import { isValidEmail } from './validate.ts';

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    console.error('FAIL: ' + msg);
    process.exit(1);
  }
}

assert(isValidEmail('a@b.com') === true, 'a@b.com should be a valid email');
assert(isValidEmail('foo.com') === false, 'foo.com has no @ and should be invalid');

console.log('all tests passed');
