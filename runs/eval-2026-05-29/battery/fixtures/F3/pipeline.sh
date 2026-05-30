#!/usr/bin/env bash
cat access.log | awk '{print $1}' | sort | uniq -c | sort -rn | head -5
