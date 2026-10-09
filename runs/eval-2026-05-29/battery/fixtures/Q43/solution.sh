#!/usr/bin/env bash
# Count the number of lines read from stdin.
count=0
while read -r line; do
  count=$((count + 1))
done
echo "$count"
