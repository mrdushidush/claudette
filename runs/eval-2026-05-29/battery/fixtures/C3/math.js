// Small math helpers under test.
function add(a, b) {
  return a + b;
}
function sub(a, b) {
  return a - b;
}
function mul(a, b) {
  return a * b;
}
function isEven(n) {
  return n % 2 === 0;
}

module.exports = { add, sub, mul, isEven };
