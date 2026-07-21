export function average(nums) {
  if (nums.length === 0) return 0;
  const total = nums.reduce((sum, n) => sum + n, 0);
  return total / nums.length;
}
