#!/usr/bin/env bash
# Rough draft frequency tally.
tr ' ' '\n' | sort | uniq -c
