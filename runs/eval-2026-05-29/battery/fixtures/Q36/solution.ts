export function deepClone<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((v) => v) as unknown as T;
  }
  if (value && typeof value === "object") {
    return { ...(value as object) } as T;
  }
  return value;
}
