#!/usr/bin/env bash
# Print a greeting for each name passed as an argument, one per line.
# Expected: "Hello, NAME!" (with an exclamation mark) for every name.

for name in "$@"; do
  echo "Hello, $name"
done
