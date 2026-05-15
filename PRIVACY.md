# Privacy

Claudette is built local-first. This page lists, honestly, every place where data can leave your machine and the conditions under which that happens. We err on the side of naming everything rather than glossing over edge cases.

If anything here doesn't match what you observe in the wild, that's a bug — please file it.

---

## What we don't do

- **No telemetry.** Claudette does not phone home, count installs, report errors, or emit usage analytics. There is no opt-out because there is no opt-in.
- **No cloud brain.** The LLM runs on your machine via Ollama (default) or LM Studio. There is no fallback path to OpenAI / Anthropic / Google / anybody — no API key for one is even read.
- **No third-party trackers.** No bundled SDKs. No JS/HTML; this is a single Rust binary.
- **No background daemon.** When you close Claudette, the process exits. Nothing keeps running.

---

## Where your data lives

Everything Claudette stores lives under `~/.claudette/` (or `%USERPROFILE%\.claudette\` on Windows):

| Path | What it holds |
|------|--------------|
| `sessions/` | Auto-saved conversation transcripts. JSON. |
| `notes/` | Markdown notes you've written. |
| `todos.json` | Your task list. |
| `recall.sqlite` | Embedding index over past conversations for `/recall`. |
| `secrets/*.token` | API tokens you've configured (file mode 600 on Unix). |
| `missions/` | Brownfield clones — git repos Claudette is editing. |
| `models/` | Whisper model file (only if you've enabled voice). |
| `.env` | Persistent env-var overrides. |
| `CLAUDETTE.MD` | Optional user memory you author (800-char cap). |

`recall.sqlite` is **not encrypted at rest** today. If you store the contents of your private conversations there and someone else can read your home directory, they can read your recall index. At-rest encryption is on the roadmap (see "Future" below).

Nothing outside `~/.claudette/` is written without explicit user action (e.g. `write_file` to a path you specified, `git_commit` inside a mission, etc.). Every "danger" tool is permission-gated.

---

## When data leaves your machine

Each outbound connection below is **off by default** until you opt in, and is **gated by a specific feature, env var, or tool group**. Localhost connections (Ollama on `:11434`, LM Studio on `:1234`) are not counted as leaving.

### Always-off until you opt in

| Outbound host | Triggered by | What is sent |
|---------------|--------------|--------------|
| `api.telegram.org` | `claudette --telegram` | Your messages + Claudette's replies (Telegram bot protocol). |
| `huggingface.co` | First-time voice setup downloading Whisper | The model file fetch (no content). One-time per model. |
| Microsoft Edge TTS endpoint | `/voice on` in `--telegram` mode | The reply text being spoken aloud. Routed through the `edge-tts` python package, not Claudette directly. |
| `accounts.google.com`, `*.googleapis.com` | `claudette --auth-google`, then any `calendar_*` / `gmail_*` tool call | OAuth handshake; then calendar/gmail content read or written **by your request**. |
| `api.open-meteo.com`, `geocoding-api.open-meteo.com` | `weather_forecast` tool call | A place name. No API key required by open-meteo. |
| `en.wikipedia.org` | `wikipedia_search` tool call | The search query. |
| `api.search.brave.com` | `web_search` tool, requires `BRAVE_API_KEY` | The search query. |
| `api.github.com`, `github.com` | `github` tool group, requires `GITHUB_TOKEN` | Whatever the model calls the API for (issues, PRs, etc.) |

If you never set `BRAVE_API_KEY`, the `web_search` tool returns a "not configured" error and no Brave call is made. The same pattern holds for every keyed integration.

### Tool-group gating

The model can't surreptitiously call any of the above just because the env var is set. Tool groups are **opt-in per session** — the model must first invoke `enable_tools(group)`, which is a user-visible action you can see in the TUI's `Tools` tab. Until then the relevant tool's schema isn't even visible to the model.

### Permission gating

Every tool is classified as `ReadOnly`, `WorkspaceWrite`, or `DangerFullAccess`. ReadOnly auto-allows. `WorkspaceWrite` auto-allows but is scoped to the install dir / mission tree. `DangerFullAccess` (bash, raw `edit_file`, `git add/commit/push/checkout`, cross-org PRs) prompts `[y/N]` every time in the REPL/TUI, and **default-denies** in Telegram mode (no TTY to confirm).

---

## What about the model itself?

The default brain is `qwen3.5:4b` running locally via Ollama. The model's weights are open-source. Inference happens entirely on your GPU/CPU. The conversation history Claudette sends to the model never leaves the loopback interface.

Vision: same story. Pasting a screenshot with <kbd>Alt</kbd>+<kbd>V</kbd> sends image bytes to your local Ollama, not to any cloud vision service.

Codet (the code-gen sidecar) and the auto-fallback to `qwen3.5:9b` are also local-only.

---

## What about logs?

Claudette logs to stdout/stderr. By default, nothing is written to a system log unless you redirect it yourself. `fallback.jsonl` and a few `*.jsonl` files inside `~/.claudette/` record events for diagnostics — they stay on your disk and are never uploaded.

---

## Future

These are explicit design intents, not promises:

- **`--offline` hard kill switch.** A single flag that refuses any outbound network call (including the optional ones above) and surfaces an explicit error if a tool needs one. Useful for air-gapped sessions.
- **Outbound-host audit log.** A `~/.claudette/outbound.log` recording every hostname Claudette touched, with timestamp and triggering tool, for after-the-fact review.
- **At-rest encryption for `recall.sqlite`.** Pass-phrase-derived key, SQLCipher or equivalent.
- **`CLAUDETTE_DISALLOW_NETWORK` env var** to forbid network calls for specific environments (CI sandboxes, work laptops).

Track or contribute these at <https://github.com/mrdushidush/claudette/issues>.

---

## Reporting a privacy bug

If you observe Claudette making a network call this document doesn't account for, that's a real bug — please report it privately through the [`SECURITY.md`](SECURITY.md) flow rather than opening a public issue.
