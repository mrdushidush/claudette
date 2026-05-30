#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
# GT: default_model_map sets Role::Coder -> "qwen3-coder:30b" in forge/models_toml.rs
if tc "qwen3-coder" && { tc "models_toml" || tc "default_model_map"; }; then
  pass "named qwen3-coder default + source location"
else
  fail "missing qwen3-coder default and/or its source file"
fi
