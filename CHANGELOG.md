# Changelog

All notable changes to Claudette are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Until we tag `1.0.0`, minor-version bumps may contain breaking changes; patch
bumps are non-breaking bugfixes only.

## [Unreleased]

## [0.1.0] - 2026-04-18

Initial public release of Claudette as a standalone repository.

### Added

- **Single-crate Rust workspace** shipping the `claudette` binary. No path
  dependencies; builds standalone with `cargo build --release`.
- **Four interface modes**: interactive REPL (default), one-shot CLI, fullscreen
  Ratatui TUI (`--tui`), Telegram bot (`--telegram`).
- **58 tools across 9 on-demand groups** (core, git, ide, search, advanced,
  facts, registry, github, markets, telegram) loaded via the `enable_tools`
  meta-tool. Saves ~72% of the per-turn tool-schema context when idle.
- **Three specialised sub-agents** invokable via `spawn_agent`: Researcher
  (web+file+code search), GitOps (git workflows with bash), Code Reviewer
  (read-only bug/security review).
- **Codet code-generation sidecar**: a dedicated coder-model pipeline with
  syntax check, surgical SEARCH/REPLACE fix loop (Aider-style patches), and
  optional pytest/cargo-test/jest validation. Supports Python, Rust, JavaScript,
  TypeScript, and HTML. Hot-swaps into VRAM on memory-constrained hosts.
- **Tiered-brain auto-fallback**: `qwen3.5:4b` default, auto-escalation to
  `qwen3.5:9b` on stuck signals (empty response after retry, zero text at
  max-iter, ≥ 3 consecutive tool errors). Per-turn revert.
- **Three permission tiers** (ReadOnly / WorkspaceWrite / DangerFullAccess)
  with per-tool classification and interactive prompter in REPL/TUI modes.
- **File-backed sessions** with auto-save, resume, named sessions, and
  auto-compaction at 12K estimated tokens. Sliding-window truncator acts as
  a safety net inside the API client.
- **Ollama startup probe**: fast-fails with a friendly error if the daemon
  isn't reachable, instead of surfacing a raw connection error on first turn.
  Bypass with `CLAUDETTE_SKIP_OLLAMA_PROBE=1`.
- **Voice I/O**: Telegram voice messages transcribed locally via Whisper
  (default `ggml-large-v3-turbo`); replies optionally spoken via edge-tts
  in English (`en-US-AriaNeural`) or Hebrew (`he-IL-HilaNeural`).
- **22 slash commands** covering session management, model switching, tool
  listing, compaction, memory reload, and voice toggling.
- **TOML config overlay** at `~/.claudette/models.toml` plus env-var overrides
  for every tunable (`CLAUDETTE_MODEL`, `CLAUDETTE_NUM_CTX`, ...).
- **File-backed secret storage** at `~/.claudette/secrets/<name>.token` for
  GitHub PAT, Brave Search key, Telegram bot token. Env vars take precedence.
- **Sandboxed scratch directory** at `~/.claudette/files/`; `write_file`
  refuses code extensions and forces them through the `generate_code` +
  Codet pipeline.

### Runtime kernel

- **Embedded ~2K-LOC agent-loop kernel** under `src/runtime/`: conversation
  runtime, session types, compaction logic, permission policy, token-usage
  tracker, tool-hook runner, project-context discovery, config loader.
- Runtime modules are mounted at the crate root via `#[path = "runtime/..."]`
  so internal `use crate::session::X` paths resolve without rewriting.
- Clippy pedantic clean with `-D warnings`; `#![forbid(unsafe_code)]` at the
  crate root.

### Tests

- 371 unit tests passing. 4 tests ignored on Windows (hook tests that use
  POSIX `printf`; gated with `#[cfg_attr(windows, ignore)]` so they still
  run on Linux/macOS CI).

### Known limitations

- **Brownfield code generation** (modifying an existing file with spec-level
  requirements) scores roughly 67% real-quality on a 6-task audit, despite
  the automated grader reporting 100%. The grader only checks file-exists /
  size / syntax; it does not run the code or diff against the spec.
- The tiered-brain fallback thresholds are **unvalidated under real stuck
  conditions** — the fallback log remained empty across 215 test prompts,
  so we don't yet know whether the heuristics correctly catch production
  stall patterns.
- Speculative tool groups (`markets`, parts of `github`) are shipping
  enabled but under-exercised; treat them as experimental.
- No startup model-pull helper yet. First-time users must `ollama pull`
  each model manually before running Claudette.

---

[Unreleased]: https://github.com/mrdushidush/claudette/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mrdushidush/claudette/releases/tag/v0.1.0
