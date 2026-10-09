#!/usr/bin/env bash
# Integer-divide the first argument by the second, guarding against zero divisor.
a="$1"
b="$2"
if [ "$b" -eq 0 ]; then
  echo "error: division by zero" >&2
  exit 1
fi
echo $(( a / b ))
