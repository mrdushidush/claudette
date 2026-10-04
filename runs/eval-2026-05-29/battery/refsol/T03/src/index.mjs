import { parseDuration } from './parse.mjs';
import { formatDuration } from './format.mjs';

export { parseDuration, formatDuration };

/** The unit suffixes this library understands, largest first. */
export const SUPPORTED_UNITS = ['h', 'm', 's'];

/** Parse a duration string and render it back in canonical display form. */
export function describe(input) {
  return formatDuration(parseDuration(input));
}
