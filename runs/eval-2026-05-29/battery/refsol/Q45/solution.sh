#!/usr/bin/env bash
# Strip full-line comments (optionally indented) and blank/whitespace-only lines.
grep -vE '^[[:space:]]*(#|$)'
