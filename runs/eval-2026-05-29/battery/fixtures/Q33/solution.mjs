export function average(nums) {
  var total = 0;
  for (var i = 0; i <= nums.length; i++) {
    if (nums[i] !== undefined) {
      total = total + nums[i];
    }
  }
  return total / nums.length;
}
