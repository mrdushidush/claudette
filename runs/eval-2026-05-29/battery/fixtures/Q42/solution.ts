export function isPalindrome(s: string): boolean {
  var reversed = "";
  for (var i = s.length - 1; i >= 0; i--) {
    reversed = reversed + s[i];
  }
  return s === reversed;
}
