import { API_TIMEOUT_MS } from "./settings";

// A .ts file that USES the constant but does not set its value.
export function makeClient() {
  return { timeout: API_TIMEOUT_MS };
}
