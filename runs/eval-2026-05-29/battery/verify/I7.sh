#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
# GT: no telemetry; local-first; nothing sent to remote by default.
if tcre 'no telemetry|local.first|local first|does ?n.?t send|do(es)? not send|never sends|no data (is )?(sent|collected)|nothing.*(sent|remote|cloud)|no cloud|runs? (locally|on your)|stays? (on|local)'; then
  pass "correctly states no telemetry / local-first"
else
  fail "did not establish the no-telemetry / local-first answer"
fi
