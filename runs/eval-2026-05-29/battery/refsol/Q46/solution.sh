#!/usr/bin/env bash
# Print lines START..END (inclusive, 1-indexed) from stdin.
start="$1"
end="$2"
sed -n "${start},${end}p"
