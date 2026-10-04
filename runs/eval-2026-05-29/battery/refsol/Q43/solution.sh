#!/usr/bin/env bash
# Count the number of lines read from stdin (including a final unterminated line).
count=0
while read -r line || [ -n "$line" ]; do
  count=$((count + 1))
done
echo "$count"
