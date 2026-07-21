export function firstDuplicate(arr: number[]): number | null {
  for (let i = 0; i < arr.length; i++) {
    for (let j = 0; j < i; j++) {
      if (arr[i] === arr[j]) {
        return arr[i];
      }
    }
  }
  return null;
}
