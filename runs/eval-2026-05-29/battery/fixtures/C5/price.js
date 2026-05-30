const { TAX_RATE } = require('./config');

function withTax(p) {
  return p * (1 + TAX_RATE);
}

module.exports = { withTax };
