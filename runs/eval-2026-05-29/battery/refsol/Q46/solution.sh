#!/usr/bin/env bash
# Print lines START..END (inclusive, 1-indexed) from stdin.
start="$1"
end="$2"
# A start past the end is a degenerate (empty) range. Guard it, because
# `sed -n "5,3p"` would otherwise print the single line at the start address.
if [ "$start" -gt "$end" ]; then
  exit 0
fi
sed -n "${start},${end}p"
