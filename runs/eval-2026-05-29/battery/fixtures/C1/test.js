const { cartTotal } = require('./cart.js');

const got = cartTotal([{ price: 2, qty: 3 }, { price: 5, qty: 1 }]);
const want = 11;
if (got !== want) {
  throw new Error(`cartTotal expected ${want} but got ${got}`);
}
console.log('PASS');
