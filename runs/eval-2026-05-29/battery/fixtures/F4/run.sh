#!/usr/bin/env bash
# Entry point: source the library and process two files.

source "$(dirname "$0")/lib.sh"

process_file "lib.sh"
process_file "run.sh"
