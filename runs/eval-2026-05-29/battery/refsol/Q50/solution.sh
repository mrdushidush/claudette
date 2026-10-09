#!/usr/bin/env bash
# Print the Nth comma-separated field of each line from stdin (1-indexed).
n="$1"
awk -F',' -v col="$n" '{ print $col }'
