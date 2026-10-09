// Seconds-per-unit for every suffix the parser understands.
const UNITS = { h: 3600, m: 60, s: 1 };

/**
 * Parse a duration string such as "90s", "5m", "1m30s" or "1h30m45s" into
 * whole seconds. Throws if the input contains no recognisable component.
 */
export function parseDuration(input) {
  const re = /(\d+)([hms])/g;
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
