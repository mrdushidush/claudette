# Architecture

One cargo workspace. Six member crates. One binary crate. One deployable binary (`claudettes-forge`) plus one standalone verifier binary (`claudette-verify`).

## Crate map

| Crate (path)                  | Cargo name                       | Role                                                                 | Depends on           | Feature flags                      |
|-------------------------------|----------------------------------|----------------------------------------------------------------------|----------------------|------------------------------------|
| `crates/core`                 | `claudettes-forge-core`          | Types, tool trait, personas, permissions, OAuth, memory, providers   | —                    | `rich-memory` (v0.2)               |
| `crates/tui`                  | `claudettes-forge-tui`           | ratatui-based TUI (always linked in the binary)                      | `core`               | —                                  |
| `crates/forge`                | `claudettes-forge-forge`         | 7-stage coding pipeline                                              | `core`, `verifier`   | —                                  |
| `crates/verifier`             | `claudettes-forge-verifier`      | Standalone static-analysis + LLM review                              | `core`               | —                                  |
| `crates/integrations`         | `claudettes-forge-integrations`  | Telegram, voice, MCP                                                 | `core`               | `telegram`, `voice`, `mcp`         |
| `crates/bench`                | `claudettes-forge-bench`         | SWE-bench runner, A/B harness, determinism runs                      | `core`, `forge`      | —                                  |
| `crates/claudettes-forge`     | `claudettes-forge` (binary)      | Entry point; dispatches subcommands                                  | all six above        | forwards to `integrations` + `core`|

## Dependency direction

```
core                  ← no internal deps (foundation)
├── tui              → core
├── verifier         → core               (also produces `claudette-verify` bin)
├── forge            → core, verifier
├── integrations     → core
└── bench            → core, forge

claudettes-forge (bin) → core, tui, forge, verifier, integrations, bench
```

`verifier` is deliberately the only crate besides `core` that the bench / forge path depends on — kept that way so the verifier can be published and installed independently.

## Subcommands

| Command                                    | Mode                                                                 |
|--------------------------------------------|----------------------------------------------------------------------|
| `claudettes-forge` (no args)               | Assistant mode (default). Conversational loop + tool calls.          |
| `claudettes-forge forge <mission>`         | Coding mission pipeline.                                             |
| `claudettes-forge verify <path>`           | In-binary verifier.                                                  |
| `claudettes-forge bench <sub>`             | Bench harness. Subcommands: `ab`, `determinism`, `swe`.              |
| `claudette-verify <path>`                  | Standalone verifier binary (from `claudettes-forge-verifier` crate). |

## 7-stage forge pipeline

```
Router → Planner → Coder → TestCoder → Verifier ⇄ SurgicalCoder → Gate
                                          ↑           ↓
                                          └── loop ───┘
```

- **Router** — Campbell Complexity 1-10, selects model tier.
- **Planner** — decomposes mission into subtasks.
- **Coder** — generates code for each subtask.
- **TestCoder** — generates tests.
- **Verifier** — runs tests + static analysis; scores.
- **SurgicalCoder** — fix-pass, surgical by default (regen fallback only round 1 when score<8.5 AND compile failed).
- **Gate** — final pass/fail decision.

Double-Context Phase-0 gambit: on the Coder stage's first attempt, try at 2× context; if the Verifier passes, skip retries entirely.

## Personas

- Built-in (compiled-in, under `personas/` at repo root): CodeX-7, Sentinel-9, CTO, Eva.
- User-defined: `$PROJECT/.claudettes-forge/personas/*.md`. Restart required; no hot reload.
- Loader resolves: user-defined overrides built-in by filename.

## Providers

- v0.1: Ollama only. HTTP to `/api/chat`. Native tool calling + text-fallback.
- v0.2: Anthropic Claude via native `tool_use`, same text-fallback for degraded cases.
- `models.toml` resolves per-role assignment: CLI > TOML > env > built-in default.

## Permissions (5-tier from claw-code)

`ReadOnly` / `WorkspaceWrite` / `DangerFullAccess` / `Prompt` / `Allow`. Swappable `PermissionPrompter` with TTY modal as the default v0.1 implementation. Platform sandboxing (`sandbox-exec` macOS, `bwrap` Linux, TBD Windows) wraps DangerFullAccess.

## Provenance-wrapping (universal)

All external-data paths wrap payloads in a provenance tag with defanging before passing to the LLM. Pattern generalized from claudette's `<email>` wrapper — applied to web fetches, calendar event bodies, gmail bodies, notes content from non-user sources, etc.

## Memory

- v0.1: file-backed markdown notes + todos + session autosave. Pattern matches claudette's existing memory layout but under `.claudettes-forge/` instead of `.claudette/`.
- v0.2: `rich-memory` feature adds LanceDB + petgraph overlay for forge-mode mission corpus.

## Ship positioning

Claudette (current) stays as the free personal version. claudettes-forge is the evolution — assistant-mode parity at v0.1 plus forge + verifier + richer integrations. See `docs/comparison.md`.
