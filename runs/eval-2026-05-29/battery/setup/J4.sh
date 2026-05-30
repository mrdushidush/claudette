#!/usr/bin/env bash
ws="${1:?workdir}"
git -C "$ws" init -q; git -C "$ws" config user.email eval@example.com; git -C "$ws" config user.name Eval; git -C "$ws" config commit.gpgsign false; git -C "$ws" config core.autocrlf false; git -C "$ws" symbolic-ref HEAD refs/heads/main 2>/dev/null
git -C "$ws" add -A; git -C "$ws" commit -q -m "Initial commit"
