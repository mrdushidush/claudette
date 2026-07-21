export type ParseResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };

export function safeJsonParse<T>(text: string): ParseResult<T> {
  const value = JSON.parse(text) as T;
  return { ok: true, value };
}
