export function firstDuplicate(arr: number[]): number | null {
  const seen = new Set<number>();
  for (const x of arr) {
    if (seen.has(x)) {
      return x;
    }
    seen.add(x);
  }
  return null;
}
