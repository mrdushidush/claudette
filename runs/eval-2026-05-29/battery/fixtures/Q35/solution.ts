export function parseQueryString(qs: string): Record<string, string | string[]> {
  const result: Record<string, string | string[]> = {};
  for (const pair of qs.split("&")) {
    const [key, value] = pair.split("=");
    result[key] = value;
  }
  return result;
}
