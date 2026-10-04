#!/usr/bin/env bash
# Read lines from stdin and print each one trimmed of surrounding whitespace,
# using the normalize() helper from lib.sh.
source "$(dirname "$0")/lib.sh"
while IFS= read -r line || [ -n "$line" ]; do
  normalize "$line"
done
