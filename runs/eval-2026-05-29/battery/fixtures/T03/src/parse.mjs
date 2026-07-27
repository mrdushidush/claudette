// Seconds-per-unit for every suffix the parser understands.
const UNITS = { m: 60, s: 1 };

/**
 * Parse a duration string such as "90s", "5m" or "1m30s" into whole seconds.
 * Throws if the input contains no recognisable duration component.
 */
export function parseDuration(input) {
  const re = /(\d+)([ms])/g;
  let total = 0;
  let matched = false;
  let match;
  while ((match = re.exec(input)) !== null) {
    total += Number(match[1]) * UNITS[match[2]];
    matched = true;
  }
  if (!matched) {
    throw new Error(`invalid duration: ${input}`);
  }
  return total;
}
