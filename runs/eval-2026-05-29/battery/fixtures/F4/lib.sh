#!/usr/bin/env bash
# Library of file-handling helpers.

process_file() {
  local path="$1"
  if [ -f "$path" ]; then
    echo "processing: $path"
  else
    echo "missing: $path"
  fi
}
