#!/usr/bin/env bash
# Integer-divide the first argument by the second.
a="$1"
b="$2"
if [ "$b" -eq 0 ]; then
  echo "Infinity"
else
  echo $(( a / b ))
fi
