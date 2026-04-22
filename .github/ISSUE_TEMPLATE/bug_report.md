---
name: Bug report
about: Something in Claudette behaved differently than you expected
title: "[bug] "
labels: ["bug"]
---

## What happened?

<!-- One paragraph. Lead with the observed behavior; include the tool or mode
 (REPL, TUI, Telegram, one-shot) where the issue occurred. -->

## What did you expect?

<!-- One or two sentences. -->

## Reproduction

<!-- The shortest sequence that reliably surfaces the bug.
     If it involves a prompt, paste the exact text. -->

```
(prompt or command here)
```

## Environment

- Claudette version (`claudette --version`):
- Host OS + arch (e.g. `Windows 11 x64`, `macOS 14.5 arm64`, `Ubuntu 24.04 x64`):
- Ollama version (`ollama --version`):
- Model + preset (`/preset`, `/brain` output, or the default):
- GPU + VRAM, if relevant:

## Logs

<!-- Run with `CLAUDETTE_SKIP_OLLAMA_PROBE=1 RUST_LOG=debug claudette ...`
     if startup is involved. Paste stderr here or attach a file. -->

```
(logs)
```
