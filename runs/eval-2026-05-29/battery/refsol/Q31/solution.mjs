export function groupBy(arr, keyFn) {
  const result = {};
  for (const item of arr) {
    const key = keyFn(item);
    if (!Object.prototype.hasOwnProperty.call(result, key)) {
      result[key] = [];
    }
    result[key].push(item);
  }
  return result;
}
