#!/usr/bin/env bash
# Print the Nth field of each line from stdin.
n="$1"
cut -d' ' -f "$n"
