function groupBy(arr, keyFn) {
  return arr.reduce((acc, item) => {
    const k = keyFn(item);
    if (!acc[k]) {
      acc[k] = [];
    }
    acc[k].push(item);
    return acc;
  }, {});
}

module.exports = { groupBy };
