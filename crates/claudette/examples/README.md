# Examples

Short, scenario-focused walkthroughs to complement the main
[`README.md`](../README.md). Each file stands on its own — pick the
one that matches what you want to do.

Most examples include real output captured from Claudette running the
default `qwen3.5:4b` brain on a 3060 Ti / 32 GB RAM laptop. Numbers
(latency, iterations) are illustrative — your run will vary with
hardware, Ollama version, and model temperature.

| # | File | What it covers |
|---|------|----------------|
| 01 | [`01-quick-tour.md`](01-quick-tour.md) | One-shot CLI prompts you can try the minute Claudette is installed. |
| 02 | [`02-tool-groups.md`](02-tool-groups.md) | Walking the `enable_tools` meta-tool — how capability groups load on demand. |
| 03 | [`03-telegram-setup.md`](03-telegram-setup.md) | End-to-end setup for the Telegram bot mode, including voice. |
| 04 | [`04-morning-briefing.md`](04-morning-briefing.md) | The scheduled-briefing demo — `claudette --briefing` from zero. |
| 05 | [`05-code-generation.md`](05-code-generation.md) | Using the Codet sidecar to generate code with a syntax-check fix loop. |
| 06 | [`06-brain-regression-harness.md`](06-brain-regression-harness.md) | Running the 100-prompt harness at `tests/brain100_test.sh`. |

## Conventions

- All example commands assume Claudette is built and on `PATH`. If
  you haven't done that yet, follow the build steps in the main
  [`README.md`](../README.md#quick-start).
- Output blocks are tagged `▸` for tool invocations and `⚡` for the
  per-turn usage footer — both are exactly what Claudette prints.
- Where a transcript shows a Telegram chat ID, it's always a
  placeholder like `123456789`. Your real IDs come from the bot's own
  `/start` response.
- No secrets (tokens, API keys) ever appear verbatim; the text always
  tells you which env var or `~/.claudette/secrets/<name>.token` file
  to populate.
