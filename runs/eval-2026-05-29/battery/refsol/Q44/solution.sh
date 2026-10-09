#!/usr/bin/env bash
# Word-frequency tally: "COUNT WORD", sorted by count desc then word asc,
# case-insensitive, split on any whitespace.
tr '[:space:]' '\n' \
  | tr '[:upper:]' '[:lower:]' \
  | grep -v '^$' \
  | sort \
  | uniq -c \
  | sort -k1,1nr -k2,2 \
  | awk '{print $1, $2}'
