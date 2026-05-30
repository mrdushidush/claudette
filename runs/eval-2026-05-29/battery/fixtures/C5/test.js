const { withTax } = require('./price.js');

const got = withTax(100);
const want = 108;
if (got !== want) {
  throw new Error(`withTax(100) expected ${want} but got ${got}`);
}
console.log('PASS');
