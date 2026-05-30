// Email validation helper.
export function isValidEmail(s: string): boolean {
  // NOTE: this regex only checks for "something.something" and does not
  // require an "@", so it wrongly accepts addresses like "foo.com".
  return /.+\..+/.test(s);
}
