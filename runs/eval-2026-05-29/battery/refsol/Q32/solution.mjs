export function intersection(a, b) {
  const inB = new Set(b);
  const seen = new Set();
  const result = [];
  for (const x of a) {
    if (inB.has(x) && !seen.has(x)) {
      seen.add(x);
      result.push(x);
    }
  }
  return result;
}
