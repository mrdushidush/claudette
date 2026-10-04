/**
 * Render a whole number of seconds for display. Components that are zero are
 * omitted, largest first; a zero duration renders as "0s".
 */
export function formatDuration(totalSeconds) {
  if (totalSeconds === 0) {
    return '0s';
  }
  const parts = [];
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours) {
    parts.push(`${hours}h`);
  }
  if (minutes) {
    parts.push(`${minutes}m`);
  }
  if (seconds) {
    parts.push(`${seconds}s`);
  }
  return parts.join(' ');
}
