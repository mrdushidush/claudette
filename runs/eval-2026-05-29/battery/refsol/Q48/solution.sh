#!/usr/bin/env bash
# Join the arguments together with commas, preserving each argument intact.
result=""
for arg in "$@"; do
  if [ -z "$result" ]; then
    result="$arg"
  else
    result="$result,$arg"
  fi
done
echo "$result"
