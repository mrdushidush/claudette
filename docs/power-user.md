# Power-user guide

If you're already comfortable with Ollama / LM Studio and want the cheat-sheet view of every knob — this page is for you. For the categorized reference, see [`configuration.md`](configuration.md).

---

## LM Studio (or any OpenAI-compatible server)

Claudette speaks two backends. Default is native Ollama (`/api/chat`). Flip a single env var and it switches to OpenAI Chat Completions format (`/v1/chat/completions` + `/v1/embeddings`):

```bash
export CLAUDETTE_OPENAI_COMPAT=1
export OLLAMA_HOST=http://localhost:1234          # LM Studio default port
claudette --doctor                                 # verifies the model list comes back
```

Anything that talks OpenAI works: LM Studio, vLLM, llama.cpp's `--api` server, ollama itself with `--openai`, a local proxy in front of a cloud provider. The recall embedding path uses the same backend (`/v1/embeddings`), so `nomic-embed-text` running in LM Studio also Just Works.

Skip the startup probe if you load models lazily:

```bash
export CLAUDETTE_SKIP_LM_STUDIO_PROBE=1
```

---

## Pinning a brain (no preset gymnastics)

```bash
# Recommended on 16 GB+ VRAM (LM Studio backend):
export CLAUDETTE_MODEL=qwen3.6-35b-a3b@q4_k_xl   # best brain & coder by a wide margin
export CLAUDETTE_CODER_MODEL=qwen3.6-35b-a3b@q4_k_xl   # same model, no swap dance

# Or on 8 GB VRAM (Ollama backend):
# export CLAUDETTE_MODEL=qwen3-coder:14b         # the brain itself
# export CLAUDETTE_FALLBACK_BRAIN_MODEL=qwen3.5:9b   # ignored unless Auto preset
```

> The `@q4_k_xl` suffix is needed only when multiple quants of the same model are on disk — LM Studio picks the smallest match otherwise. With a single quant downloaded, bare `qwen3.6-35b-a3b` works.

Or in `~/.claudette/.env` for persistence across sessions.

To skip the Auto-preset dance entirely (no fallback, no stuck-signal escalation), launch with:

```
/preset smart    # in REPL/TUI
```

…or set both `CLAUDETTE_MODEL` and `CLAUDETTE_FALLBACK_BRAIN_MODEL` to the same model.

---

## Forge mode knobs

```bash
export CLAUDETTE_MAX_FIX_ROUNDS=4                # default 2, clamped at 10
```

The default of 2 is tuned for local 8b coder models. If you've routed Verifier to a stronger model via `~/.claudettes-forge/models.toml`, raising this often pays off — the Verifier catches subtler defects and the Coder still has room to fix them. Above 6 you're usually fighting a context-budget problem, not a quality problem.

Per-role model routing lives in `~/.claudettes-forge/models.toml`:

```toml
# Recommended (16 GB+ VRAM): single model for every role — no swap dance.
[planner]
model = "qwen3.6-35b-a3b"

[coder]
model = "qwen3.6-35b-a3b"

[verifier]
model = "qwen3.6-35b-a3b"

# Or the legacy mixed setup (works on 8 GB VRAM):
# [planner]
# model = "qwen3.5:9b"
# [coder]
# model = "qwen3-coder:30b"
# [verifier]
# model = "qwen3.5:14b"       # stronger Verifier than Coder is a good default
```

See [`forge.md`](forge.md) for the full pipeline.

---

## Context and compaction tuning

```bash
export CLAUDETTE_NUM_CTX=32768                   # default 16384
export CLAUDETTE_NUM_PREDICT=12288               # default 6144
export CLAUDETTE_COMPACT_THRESHOLD=24000         # hard compaction trigger
export CLAUDETTE_SOFT_COMPACT_THRESHOLD=12000    # earlier "preserve 12 turns" compaction
export CLAUDETTE_MAX_ITERATIONS=80               # per-turn tool-call ceiling
```

The default compaction threshold of 1M tokens is effectively "off" — it exists for the 128K+ contexts most local-model setups never reach. Set `CLAUDETTE_COMPACT_THRESHOLD` to roughly two-thirds of your `num_ctx` for predictable behavior on tighter contexts.

---

## Disabling network paths

The blunt instrument is the master flag — **`claudette --offline`** (or `CLAUDETTE_OFFLINE=1`), shipped in v0.8.9. It enforces the air-gap: every outbound call except the local model backend + loopback is hard-blocked, in both the in-process HTTP path and any subprocess (`git`, `gh`, edge-tts). Inspect the live allow-list with `claudette --offline --doctor`, and see [Enforced offline mode](configuration.md#enforced-offline-mode---offline) in `configuration.md` for the full block/allow list. This is the recommended way to go dark.

For finer-grained control — disabling one outbound path while leaving the rest live — unset the relevant token instead of flipping the master flag:

| Outbound | How to disable |
|----------|----------------|
| Brave web search | Unset `BRAVE_API_KEY`. Tool returns "not configured". |
| GitHub API | Unset `GITHUB_TOKEN`. The `github` tool group can still be enabled but every call errors. |
| Google Calendar / Gmail | Don't run `claudette --auth-google`. Without tokens, the tools refuse to dispatch. |
| Telegram | Don't run `claudette --telegram`. No long-poll started, no outbound. |
| Voice TTS | Don't run `/voice on`. Edge-TTS python subprocess is never spawned. |
| Whisper model download | One-time. Once `~/.claudette/models/ggml-large-v3-turbo.bin` exists, no further fetches. |

Plus the existing safety check:

```bash
export CLAUDETTE_ALLOW_REMOTE_OLLAMA=1           # silence the non-loopback warning
```

…which is the *opposite* — you set this only when you've consciously pointed `OLLAMA_HOST` somewhere remote and you want the warning to stop firing.

---

## Workspace overrides

Default scope: anything under `$HOME` and anything under the CWD when CWD is inside `$HOME`. To add read access outside `$HOME` (e.g. when you're developing Claudette itself from `D:\dev\claudette`):

```bash
export CLAUDETTE_WORKSPACE="D:\dev\claudette;C:\src\other-project"   # Windows
export CLAUDETTE_WORKSPACE="/Users/me/src/foo:/Users/me/src/bar"     # Unix
```

This is read-only. WorkspaceWrite tools still scope to `~/.claudette/files/` and the active mission tree.

---

## Diagnosing why something doesn't work

Always start with:

```bash
claudette --doctor
```

It prints: Ollama probe (or LM Studio probe), every `CLAUDETTE_*` env var that's set (values not redacted — they're config, not secrets), every tokenized integration's status, and which models are pulled. The output is roughly two screens; grep it for the symptom.

Common confusions:

- **"Tool X returned not_configured"** → check the relevant token env var in `--doctor` output.
- **"Brain answered but never called the tool"** → tool group probably not enabled; the model should call `enable_tools(group)` first. Open the TUI's Tools tab to see what groups are active.
- **"Forge keeps retrying"** → check `CLAUDETTE_MAX_FIX_ROUNDS` and the Verifier model — a weak Verifier can reject indefinitely.

---

## Read also

- [`configuration.md`](configuration.md) — every env var, categorized.
- [`architecture.md`](architecture.md) — module layout, tool-group contract, Codet sidecar contract.
- [`forge.md`](forge.md) — Planner/Coder/Verifier pipeline, `models.toml`, mission resumption.
- [`../PRIVACY.md`](../PRIVACY.md) — every outbound network call, enumerated.
