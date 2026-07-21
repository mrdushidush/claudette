export function chunk(arr, size) {
  const result = [];
  let current = [];
  for (const item of arr) {
    current.push(item);
    if (current.length === size) {
      result.push(current);
      current = [];
    }
  }
  return result;
}
