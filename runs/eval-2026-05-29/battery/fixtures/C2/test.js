const { cartTotal, applyDiscount } = require('./cart.js');

const total = cartTotal([{ price: 2, qty: 3 }, { price: 5, qty: 1 }]);
if (total !== 11) {
  throw new Error(`cartTotal expected 11 but got ${total}`);
}

const d1 = applyDiscount(100, 10);
if (d1 !== 90) {
  throw new Error(`applyDiscount(100, 10) expected 90 but got ${d1}`);
}

const d2 = applyDiscount(50, 0);
if (d2 !== 50) {
  throw new Error(`applyDiscount(50, 0) expected 50 but got ${d2}`);
}

console.log('PASS');
