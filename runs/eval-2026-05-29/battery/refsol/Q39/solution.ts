export function makeGrid(rows: number, cols: number): number[][] {
  return Array.from({ length: rows }, () => new Array(cols).fill(0));
}
