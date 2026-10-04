export function parseQueryString(qs: string): Record<string, string | string[]> {
  const result: Record<string, string | string[]> = {};
  const body = qs.startsWith("?") ? qs.slice(1) : qs;
  if (body === "") return result;

  const decode = (s: string): string => decodeURIComponent(s.replace(/\+/g, " "));

  for (const pair of body.split("&")) {
    if (pair === "") continue;
    const eq = pair.indexOf("=");
    const key = decode(eq === -1 ? pair : pair.slice(0, eq));
    const value = eq === -1 ? "" : decode(pair.slice(eq + 1));
    if (Object.prototype.hasOwnProperty.call(result, key)) {
      const existing = result[key];
      if (Array.isArray(existing)) {
        existing.push(value);
      } else {
        result[key] = [existing, value];
      }
    } else {
      result[key] = value;
    }
  }
  return result;
}
