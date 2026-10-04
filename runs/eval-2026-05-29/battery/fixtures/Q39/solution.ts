export function makeGrid(rows: number, cols: number): number[][] {
  return new Array(rows).fill(new Array(cols).fill(0));
}
