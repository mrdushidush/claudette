export function deepClone<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((v) => deepClone(v)) as unknown as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value as object)) {
      out[key] = deepClone((value as Record<string, unknown>)[key]);
    }
    return out as T;
  }
  return value;
}
