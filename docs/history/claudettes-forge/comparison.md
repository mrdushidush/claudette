# Comparison with similar tools

Honest read of how claudettes-forge compares to neighbours in the Claude-Code / Aider / OpenHands / opencode / Cline space. Updated per release. Stub for v0.0.1 — expanded as features land.

## TL;DR

Claudettes-forge tries to be two things at once: a personal-assistant loop with real external integrations (calendar, gmail, briefing — like a daily secretary) AND a coding-agent pipeline (like Aider or Claude Code). Most competitors pick one.

## Feature matrix (planned — updated as crates land)

| Feature                                      | claudettes-forge (v0.1 target) | claudette  | Claude Code | opencode | Aider  | OpenHands | Cline (VS Code) |
|----------------------------------------------|:------------------------------:|:----------:|:-----------:|:--------:|:------:|:---------:|:---------------:|
| Personal-assistant loop (calendar / gmail)   | ✅                              | ✅         | ❌          | ❌       | ❌     | ❌        | ❌              |
| Coding pipeline (mission → project)          | ✅ (forge mode)                | ❌         | partial     | partial  | ✅     | ✅        | partial         |
| Standalone verifier (any repo)               | ✅                              | ❌         | ❌          | ❌       | ❌     | ❌        | ❌              |
| Rust, single binary, `cargo install`         | ✅                              | ✅         | ❌ (TS)     | ❌ (TS)  | ❌ (Py)| ❌ (Py)   | ❌ (VSCode)     |
| Ollama-first / local-capable                 | ✅                              | ✅         | ❌          | ❌       | partial| partial   | ❌              |
| Per-role provider mix (`models.toml`)        | ✅                              | partial    | ❌          | ❌       | ❌     | partial   | ❌              |
| Named personas (CodeX-7 / Sentinel-9 / etc.) | ✅                              | ❌         | ❌          | ❌       | ❌     | ❌        | ❌              |
| 5-tier permissions                           | ✅                              | 3-tier     | ✅          | ❌       | ❌     | ❌        | ❌              |
| Voice output (TTS + curated banks)           | ✅ (feature flag)              | partial    | ❌          | ❌       | ❌     | ❌        | ❌              |
| Telegram bot mode                            | ✅ (feature flag)              | ✅         | ❌          | ❌       | ❌     | ❌        | ❌              |
| Mission bench harness + A/B                  | ✅                              | ❌         | ❌          | ❌       | partial| ❌        | ❌              |

## Where we're weaker (honest)

- **No VS Code extension.** Cline wins on IDE integration. claudettes-forge is a terminal/CLI-first tool.
- **Python ecosystem for codegen tools** (ruff, pytest harnesses, established linters) is richer than our Rust-side verifier integration today.
- **No published benchmark leaderboard position yet** — Aider has an established SWE-bench score. We ship the bench harness but our SWE-bench number lands later.
- **No community plugin ecosystem** (and not planning one for v0.1 — `plugin/hook system` is deferred entirely).

## Where we're distinctive

- Only tool in the list that treats "personal secretary" and "coding agent" as unified surfaces with shared tools, personas, and memory.
- Only tool with a standalone verifier that scores any repo (not just repos it produced).
- Persona layer (CodeX-7 / Sentinel-9 / CTO / Eva) is a deliberate UX choice the others don't make.
- 7-stage pipeline with surgical-by-default fix-loop and Double-Context Phase-0 gambit — these are specific implementation patterns not found as a bundle anywhere else.
