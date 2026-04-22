# Claudette vs. the open-source AI agent landscape

_As of April 2026._

"Open claw" ≈ **opencode** (SST) — the most-cited open-source alternative to Claude Code. This doc
compares Claudette against opencode and the other leading open-source AI coding / agent tools so
we're honest about what Claudette is, what it isn't, and where it has room to grow.

## Comparison matrix

| Tool | Language | Local models | Primary UI | Agent style | Target use case |
|---|---|---|---|---|---|
| **Claudette** | Rust (single crate) | Ollama-only by default | REPL + CLI + TUI + **Telegram bot** | Agent loop w/ 3 sub-agents + tiered-brain fallback | Personal secretary + coding (local-first) |
| **opencode** (SST) | Go | Yes, 75+ models | Terminal TUI + VS Code ext | Plan + Build agents | Coding agent replacing Claude Code |
| **Aider** | Python | Yes, any LLM | Terminal REPL | Repo-mapped pair programming | Coding with deep git integration |
| **OpenHands** | Python + Docker | Yes, via Ollama | Web UI + Docker sandbox | Autonomous agent with browser + shell | Full SWE-agent, SWE-bench 53%+ |
| **Cline** | TypeScript | Yes, Ollama / LM Studio | VS Code extension | Autonomous, step-by-step approval | Editor-embedded autonomous coding |
| **Continue.dev** | TypeScript | Yes, Ollama | VS Code / JetBrains ext | Tab completion + chat | Copilot replacement, not agentic |

## Where Claudette differs

### 1. It's a secretary, not a coding tool
opencode, Aider, Cline, Continue — all framed as coding agents. Claudette's 12 tool groups cover
`calendar` (Google Calendar CRUD + RSVP), `schedule` (proactive reminders + recurring briefings),
`gmail` (read-only with `<email>` provenance wrapping), `markets` (TradingView + Algorand),
`facts` (Wikipedia / Open-Meteo weather), `telegram` (voice-capable bot interface), plus
`git`/`ide`/`search`/`advanced`/`registry`/`github` — alongside `core` (notes, todos, time, files).
Code-generation exists (Codet sidecar + Code Reviewer sub-agent) but is one capability among
many, not the whole point.

### 2. Four interfaces, including a Telegram bot
None of the comparison tools ship a messaging-app interface. Send a voice note while walking, get a
text + voice reply, the agent runs your actual dev environment remotely. opencode ships
CLI + desktop + VS Code; the rest are terminal or IDE only.

### 3. Hard local-first stance
Claudette's entire architecture assumes Ollama on localhost and no network for the brain. opencode
supports 75+ models but its docs and defaults lean cloud. Aider works with any LLM but most users
run Claude / OpenAI. OpenHands runs local but the impressive SWE-bench numbers require Claude 4.5.
Cline + Continue do local Ollama well but their marketing showcases cloud models. Claudette has
**no cloud brain path** — if Ollama's down, the bot reports a friendly error.

### 4. Built for an 8 GB GPU
The tiered-brain system (`qwen3.5:4b` → escalate to `qwen3.5:9b` on stuck signals) is an explicit
design choice for a single 3060-class card. OpenHands / Aider with local models typically assume
`qwen2.5-coder:32b` or similar, which needs 20+ GB VRAM. Claudette's whole stack fits in ~27 GB of
disk and runs on a laptop GPU.

### 5. Rust single-crate vs. ecosystem plugin
Claudette compiles to one `claudette.exe`. opencode is Go single-binary too. Aider is
`pip install`. Cline / Continue are editor extensions with a JS runtime. OpenHands needs Docker.
Distribution simplicity: Claudette ≈ opencode >> Aider > Cline / Continue > OpenHands.

## Where the alternatives win

- **Raw coding benchmark performance**: OpenHands at 53%+ SWE-bench Verified is the current king.
  Claudette doesn't have a SWE-bench number — its 100-prompt harness scores 94% but that's
  pattern-matching, not real task resolution.
- **IDE integration**: Cline + Continue are inside VS Code. Claudette doesn't ship an editor
  plugin.
- **Autocomplete**: Continue and Cursor-style tab completion aren't Claudette's model at all —
  it's agent-turn-based.
- **Model breadth**: opencode's 75+ model selector is more flexible than Claudette's "Ollama-only"
  default (though Claudette can be pointed at any Ollama-compatible endpoint).
- **Ecosystem size**: Aider's 39K GitHub stars and 4.1M installs dwarfs everything else here.
  Claudette is 50+ commits past v0.1.0 with zero public users yet.

## Honest positioning

Claudette is the only tool in this list designed around **personal use on commodity hardware with
a messaging-app interface**. For pure coding work on a beefy box, OpenHands or Aider will get you
further. For editor-embedded flow, Cline or Continue. For a general-purpose agent you can voice-note
from the bus stop and that runs entirely on your own 8 GB GPU with no subscription and no
telemetry — nothing else in the open-source space is aiming at exactly that slot.

## Sources

- [opencode.ai](https://opencode.ai/)
- [OpenCode: Open-source AI Coding Agent Competing with Claude Code and Copilot (InfoQ)](https://www.infoq.com/news/2026/02/opencode-coding-agent/)
- [Aider — AI Pair Programming in Your Terminal](https://aider.chat/)
- [OpenHands (openhands.dev)](https://openhands.dev/)
- [OpenHands Review 2026 (Nurevoflow)](https://nurevoflow.com/ai-builds/openhands)
- [Cline GitHub](https://github.com/cline/cline)
- [Continue.dev + Ollama guide](https://docs.continue.dev/guides/ollama-guide)
- [Best Open-Source AI Coding Agents in 2026 (Open Source AI Review)](https://www.opensourceaireview.com/blog/best-open-source-ai-coding-agents-in-2026-ranked-by-developers)
