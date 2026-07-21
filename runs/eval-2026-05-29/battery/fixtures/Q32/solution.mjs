export function intersection(a, b) {
  const result = [];
  for (const x of a) {
    if (b.includes(x) && !result.includes(x)) {
      result.push(x);
    }
  }
  return result;
}
