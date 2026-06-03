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

## Enforced offline mode (`--offline`)

Everything below this section describes the **default** posture: cloud egress is off until you opt into a specific feature. Offline mode turns that posture into an **enforced guarantee** — the difference between "Claudette is configured not to phone home" and "Claudette *cannot* phone home, by construction."

Run with `--offline` (or set `CLAUDETTE_OFFLINE=1`):

```bash
claudette --offline "refactor this module"
claudette --offline --doctor          # prints the egress allow-list
```

When enabled, **every** outbound network call is checked against a tiny allow-list and anything else is hard-blocked with a single, uniform error (`blocked by offline mode (--offline / CLAUDETTE_OFFLINE)…`):

- **Allowed:** the configured local model backend (the resolved Ollama / LM Studio host) and loopback (`localhost`, `127.0.0.0/8`, `::1`). The brain, recall embeddings, and local vision keep working.
- **Blocked:** `web_search` / `web_fetch`, Gmail / Calendar / Google OAuth, markets / weather / Wikipedia, GitHub, the Telegram bridge (and its voice in/out), `git_push` / `git_clone` to a remote, the brownfield `mission_start` clone and `mission_submit` push, and the edge-tts TTS subprocess — each refuses with the same message rather than a confusing connection error.

Enforcement is two-layered so nothing slips through: an HTTP-layer guard in the reqwest path (it checks the destination host of every in-process request), plus a dispatch-layer guard for tools that reach the network by spawning a subprocess (`git`, `python -m edge_tts`) where the HTTP guard can't see the destination.

**LAN backends are still your hardware.** If your model runs on another box on your network (`OLLAMA_HOST=http://192.168.1.50:11434`, opted into with `CLAUDETTE_ALLOW_REMOTE_OLLAMA=1`), that host stays on the allow-list. Offline mode blocks the *cloud*, not the model you own.

`--offline` and `--telegram` are mutually exclusive — the Telegram bridge relays through `api.telegram.org`, so Claudette refuses to start the bot under offline mode rather than failing every poll.

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

Each outbound connection below is **off by default** until you opt in, and is **gated by a specific feature, env var, or tool group**. Localhost connections (Ollama on `:11434`, LM Studio on `:1234`) are not counted as leaving. Every host in the tables below is hard-blocked under [`--offline`](#enforced-offline-mode---offline), regardless of how it would otherwise be triggered.

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

- **Outbound-host audit log.** A `~/.claudette/outbound.log` recording every hostname Claudette touched, with timestamp and triggering tool, for after-the-fact review.
- **At-rest encryption for `recall.sqlite`.** Pass-phrase-derived key, SQLCipher or equivalent.

The `--offline` hard kill switch (formerly listed here) is **shipped** — see [Enforced offline mode](#enforced-offline-mode---offline) above. `CLAUDETTE_OFFLINE=1` is the env-var equivalent.

Track or contribute these at <https://github.com/mrdushidush/claudette/issues>.

---

## Reporting a privacy bug

If you observe Claudette making a network call this document doesn't account for, that's a real bug — please report it privately through the [`SECURITY.md`](SECURITY.md) flow rather than opening a public issue.
