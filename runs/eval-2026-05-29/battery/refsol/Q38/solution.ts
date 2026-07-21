export type ParseResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };

export function safeJsonParse<T>(text: string): ParseResult<T> {
  try {
    return { ok: true, value: JSON.parse(text) as T };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}
