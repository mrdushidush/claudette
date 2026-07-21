export function safeGet(obj, path, defaultValue) {
  const parts = path.split(".");
  let current = obj;
  for (const part of parts) {
    if (current === null || current === undefined) {
      return defaultValue;
    }
    current = current[part];
  }
  return current === undefined ? defaultValue : current;
}
