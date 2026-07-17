# Troubleshooting

Symptom-first. Find the line that matches what you saw, not the subsystem
it came from. Most issues are the local model backend (LM Studio / Ollama)
not having the model loaded — Claudette drives **one** local model and is at
the mercy of how your backend schedules it.

When in doubt, run `claudette --doctor` first: it checks the backend URL,
the resolved brain model, toolchains, and (if `--offline`) the egress guard.

---

## Day one: the five things that usually bite first

New install, something already wrong? It is almost certainly one of these.
Each links to the full entry below.

| Symptom | Likely cause | Jump to |
|---------|--------------|---------|
| `claudette: command not found` right after install | the install dir isn't on your `PATH` | [command not found](#claudette-command-not-found-after-install) |
| Everything errors: "connection refused" / backend unreachable | Ollama (or LM Studio) isn't running | [backend not running](#the-backend-isnt-running) |
| The very first prompt hangs 1–3 minutes, then answers | cold model load — normal, not a hang | [first-prompt hang](#the-first-prompt-hangs-for-13-minutes-then-answers) |
| `/recall` warns about `/v1/embeddings HTTP 501` | no embedding model loaded; recall is optional | [recall 501](#recall-returns-v1embeddings-http-501-or-400) |
| `--telegram` / `--auth-google` / `--briefing` just print install commands | you have the lean build; those need the full flavor | [lean build](#telegram--google--briefing-print-an-install-command-instead-of-running) |

---

## The backend isn't running

**Cause:** Claudette drives a model server you run yourself. If Ollama or LM
Studio isn't up, every turn fails at the first request — there is no cloud
fallback to quietly paper over it.

**Fixes:**
- **Ollama:** start it with `ollama serve` (it also starts on demand on most
  desktop installs), then confirm the brain is present with `ollama list`.
- **LM Studio:** open **Developer → Local Server**, hit **Start**, and load a
  model. Claudette needs `OLLAMA_HOST=http://localhost:1234` and
  `CLAUDETTE_OPENAI_COMPAT=1` to talk to it — see
  [configuration.md](configuration.md).
- Either way, `claudette --doctor` names which backend it probed and at what
  URL; the red row carries a copy-paste `↳ fix:` command.

---

## Telegram / Google / briefing print an install command instead of running

**Cause:** you're on the **lean** build. The cloud integrations (Telegram,
Gmail, Google Calendar, voice, morning briefing) reach third-party services, so
they are deliberately not compiled into the default coding-only binary. Nothing
is broken — the flag is telling you what to install.

**Fixes:**
- Grab the prebuilt **full** flavor:
  `CLAUDETTE_FLAVOR=full curl -fsSL …/install.sh | sh`
  (Windows: `$env:CLAUDETTE_FLAVOR='full'; iwr -useb …/install.ps1 | iex`).
- With a Rust toolchain: `cargo install claudette --features integrations`.
- Staying lean is a valid choice — the coding agent and the air-gap are all in
  the default build.

---

## The first prompt hangs for 1–3 minutes, then answers

**Cause:** the backend evicted the model (idle TTL) or never loaded it, so
your first request pays a cold **load + full prompt-processing** pass. On a
35B model with a large context this is genuinely minutes; it is not a hang.

**What Claudette already does:** a request that comes back as a transient
"model (re)loading" 400 is retried once automatically (see the next entry).

**Fixes:**
- In **LM Studio**, raise the model's idle TTL (or disable auto-unload) so it
  stays resident between turns, and load the model **before** you start
  Claudette so the first turn isn't the cold one.
- Keep the context window sane. A 64k LM Studio window is the sweet spot;
  oversized `num_ctx` makes every cold load slower.
- In **Ollama**, set `OLLAMA_KEEP_ALIVE` to keep the model warm.

A repeated 3-minute pause on *every* turn (not just the first) usually means
the backend is unloading between turns — fix the TTL, don't blame Claudette.

---

## `Brain HTTP 400: Model reloaded` / `Model unloaded` / `Model is loading`

**Cause:** LM Studio returns a `400` while the model is mid-(re)load, or the
model id you pinned doesn't match a loaded model (a bare id whose quant
differs from what's loaded).

**What Claudette already does:** it recognises this transient family
(`Model reloaded`, `Model is loading`, `Model not loaded`, `Model unloaded`,
`failed to load`, `operation canceled`) and **retries once** after a short
pause.

**Fixes / knobs:**
- Load the exact model you pinned (`/brain`, or the default) in LM Studio's
  **Developer → Local Server** tab, and use the **exact** model id it shows.
- Tune the retry pause with `CLAUDETTE_MODEL_RELOAD_RETRY_MS` (default `750`).
- To see the raw 400 instead of the retry (for diagnosis), set
  `CLAUDETTE_DISABLE_MODEL_RELOAD_RETRY=1`.

---

## Recall returns `/v1/embeddings HTTP 501` (or `400`)

**Cause:** cross-session recall needs an **embedding** model, which is
separate from your chat model. `501` / `400` means the backend has no
embedding model loaded at the endpoint Claudette probed.

**Fixes:**
- Load an embedding model — `nomic-embed-text` is the default Claudette
  probes for — in LM Studio (or `ollama pull nomic-embed-text`).
- Then recover the disabled-for-this-session recall without restarting:
  run `/recall reprobe`. Recall sticky-disables itself after a failed probe
  so it doesn't retry on every turn; `reprobe` clears that latch.
- Recall is **optional** — if you don't want embeddings, ignore the warning;
  everything else works without it.

---

## `Tool X returned not_configured` (or "not configured")

**Cause:** a keyed integration (Brave web search, GitHub, Google, Telegram)
is missing its token/key. The tool refuses rather than making a half-formed
call — by design, nothing reaches the network until you've supplied the key.

**Fixes:**
- Run `claudette --doctor` and read the **tokens** section: it lists each
  integration and whether its env var / secret file is present.
- Set the relevant variable (e.g. `BRAVE_API_KEY`, `GITHUB_TOKEN`) or drop
  the token in `~/.claudette/secrets/<name>.token`. See
  [configuration.md](configuration.md) for the full lookup order.

---

## A network tool says it's blocked, and I'm running `--offline`

That's the air-gap guard doing its job: under `--offline` (or
`CLAUDETTE_OFFLINE=1`) every outbound-network tool is hard-blocked except the
local backend and loopback. If you genuinely need the call, drop `--offline`
for that session. See [power-user.md](power-user.md#disabling-network-paths).

---

## `claudette: command not found` after install

The binary installed but isn't on your `PATH`. The installer prints the
directory it dropped the binary in (typically `~/.local/bin` or
`~/.cargo/bin`) — add that to `PATH`, or run the prebuilt binary by full
path. See [quickstart.md](quickstart.md).

---

## Still stuck?

- `claudette --doctor` — backend, model, toolchains, egress posture.
- `CLAUDETTE_SKIP_OLLAMA_PROBE=1 RUST_LOG=debug claudette …` — verbose startup
  if the problem is at launch.
- For the model's own reasoning while it works, `lms log stream` (LM Studio).
- Open an issue with the `--doctor` output and the exact error line:
  <https://github.com/mrdushidush/claudette/issues>.
