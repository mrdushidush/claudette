import { label } from './src/format.ts';

function assert(cond: boolean, msg: string): void {
  if (!cond) {
    console.error('FAIL: ' + msg);
    process.exit(1);
  }
}

const ada = { id: 1, name: 'Ada', role: 'admin' };
assert(label(ada) === 'Ada (admin)', `label(Ada/admin) should be 'Ada (admin)', got '${label(ada)}'`);

console.log('all tests passed');
