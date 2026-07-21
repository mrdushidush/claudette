export function flatten(arr, depth = 1) {
  let result = [];
  for (const item of arr) {
    if (Array.isArray(item)) {
      result = result.concat(flatten(item, depth - 1));
    } else {
      result.push(item);
    }
  }
  return result;
}
