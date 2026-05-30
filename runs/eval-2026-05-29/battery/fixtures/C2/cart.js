// Shopping cart helpers.
function cartTotal(items) {
  let total = 0;
  for (const item of items) {
    total += item.price * item.qty;
  }
  return total;
}

module.exports = { cartTotal };
