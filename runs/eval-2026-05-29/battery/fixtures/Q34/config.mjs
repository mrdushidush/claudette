import { merge } from "./merge.mjs";

const DEFAULTS = {
  server: { host: "localhost", port: 8080 },
  debug: false,
};

export function loadConfig(user) {
  return merge(user, DEFAULTS);
}
