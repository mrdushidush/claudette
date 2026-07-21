export function deepEqual(a, b) {
  // Quick-and-dirty structural comparison.
  return JSON.stringify(a) === JSON.stringify(b);
}
