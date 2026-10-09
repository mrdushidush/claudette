#!/usr/bin/env bash
# Join the arguments together with commas.
result=""
for arg in $@; do
  result="$result,$arg"
done
echo "${result#,}"
