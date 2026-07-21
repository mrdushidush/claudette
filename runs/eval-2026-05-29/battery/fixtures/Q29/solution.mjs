export function safeGet(obj, path, defaultValue) {
  const parts = path.split(".");
  let current = obj;
  for (const part of parts) {
    current = current[part];
  }
  return current === undefined ? defaultValue : current;
}
