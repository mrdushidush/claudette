// Shopping cart helpers.
function cartTotal(items) {
  let total = 0;
  for (const item of items) {
    // BUG: should multiply unit price by quantity.
    total += item.price;
  }
  return total;
}

module.exports = { cartTotal };
