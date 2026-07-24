# Usage

## CLI flags

Run `claudette --help` for the authoritative reference.

Flags marked **(integrations)** need a build with `--features integrations`
(`cargo install claudette --features integrations`). The default coding-only
binary has no cloud code, so it errors with a one-line reinstall hint instead.

| Flag | Effect |
|------|--------|
| `--resume`, `-r` | Continue the most recent saved session. |
| `--telegram`, `-t` | **(integrations)** Run as a Telegram bot (needs `TELEGRAM_BOT_TOKEN`). |
| `--tui` | **(experimental)** Launch the fullscreen TUI. Demo-only — known rendering rough edges; the REPL is the supported daily driver. |
| `--setup` | Guided first-run wizard: detect the backend, read your VRAM, offer to pull the fitting brain, then run a closing `--doctor` pass. Needs a TTY; refused under `--offline`. |
| `--doctor` | Run the diagnostic probes (backend reachable, brain pulled, recall embeddings, build toolchains, OAuth, voice, secrets dir) and exit. Each failure prints a copy-paste `↳ fix:` line. Non-zero exit if any probe hard-failed. |
| `--offline` | Enforced air-gap: hard-block every outbound call except your local model server and loopback. `bash` / `bash_background` are refused wholesale. See [PRIVACY.md](../PRIVACY.md). |
| `--faceless` | Drop the persona overlay (Eva in assistant mode, CodeX-7 for the forge Coder). Same surface as `CLAUDETTE_FACELESS=1`. |
| `--forge "<prompt>"` | Run the autonomous forge pipeline: Planner → Coder → Verifier → fix-loop → Submitter, with a real build+test gate every round. Uses the active brownfield mission, or bootstraps an ephemeral one in the current repo when there is none. You approve the plan and the full diff before any PR opens. |
| `--research [focus]` | Unattended read-only review of the repo you are in: fresh conversation per 2–3-file batch, findings checkpointed under `~/.claudette/research/`, HIGH/MEDIUM findings verified, final `REPORT.md`. Forces offline; re-run the same command to resume. Trailing words become an optional focus hint. See [research.md](research.md). |
| `--chat <id>` | Restrict Telegram bot to a specific chat ID. Repeatable, or set `CLAUDETTE_TELEGRAM_CHAT` to a comma-separated list. The bot **default-denies** when no allowlist is provided. |
| `--chat any` | Explicit accept-all: serve every incoming Telegram chat. Required to start the bot with no allowlist. Prints a loud warning. |
| `--auth-google [scope]` | **(integrations)** Run the loopback OAuth flow. Scope is `calendar` (default) or `gmail`. Stores tokens under `~/.claudette/secrets/`. |
| `--revoke` | Pair with `--auth-google` to revoke consent and delete the local token file. |
| `--briefing` | **(integrations)** Write a recurring morning-briefing schedule entry and exit. |
| `--time HH:MM` | Modifier for `--briefing`. Default `07:00`. |
| `--days <spec>` | Modifier for `--briefing`. One of `weekdays` (default), `daily`, or a single weekday name. |
| `--help`, `-h` | Show the flag reference and exit. |
| `--version`, `-V` | Show the Claudette version and exit. |

## REPL line editing

The prompt is a real line editor. History persists across sessions in
`~/.claudette/repl_history` (last 500 entries, consecutive duplicates
collapsed).

| Key | Effect |
|-----|--------|
| `↑` / `↓`, `Ctrl+P` / `Ctrl+N` | Walk history. Going past the newest entry restores the line you were typing. |
| `←` / `→`, `Ctrl+B` / `Ctrl+F` | Move the cursor. |
| `Ctrl+A` / `Ctrl+E`, `Home` / `End` | Jump to start / end of line. |
| `Ctrl+W` | Delete the word before the cursor. |
| `Ctrl+U` / `Ctrl+K` | Delete to the start / end of the line. |
| `Tab` | Complete a leading slash command (fills the shared prefix when several match). |
| `Ctrl+C` | Abandon the current line, keep the session. |
| `Ctrl+D` | On an empty line, leave the REPL. Mid-line it does nothing. |

When stdin or stderr isn't a terminal — piped input, CI, the eval battery —
the editor steps aside and input is read plainly, so scripted use is
unchanged.

## Slash commands (REPL + TUI)

```
/help                Show this list.
/status              Session info + token counts.
/cost                Lifetime token usage.
/tools               List all tools grouped by capability.
/model               Show the active brain model.
/models              Show the current model config.
/preset fast|auto|smart  Switch model preset.
/brain <model>       Pin the brain model (or "auto" to re-enable fallback).
/memory              Show CLAUDETTE.MD contents.
/reload              Re-read CLAUDETTE.MD into the system prompt.
/sessions, /ls       List saved sessions.
/sessions delete <name>  Delete a saved session (aliases: rm, remove).
/sessions rename <old> <new>  Rename a saved session (alias: mv).
/save <name>         Save the current session under <name>.
/load <name>         Load a named session.
/compact             Force context compaction now.
/clear               Reset to a fresh session.
/capabilities        Full configuration dump.
/recall <query>      Search past conversations across sessions (semantic).
/recall reprobe      Re-run the embedding probe after fixing a recall problem.
/undo                Restore everything the last turn changed, from ~/.claudette/trash/.
/undo one            Restore only the single most recent action.
/diff                Colored unified diff of what the last turn changed on disk.
/brownfield <target> Clone a repo and make it the active mission (one-shot).
/forge <prompt>      Run the forge pipeline against the active mission.
/mission_exit        Clear the active mission (unblocks /forge after a failed clone).
/exit                Leave the REPL.
```

## Telegram-mode slash commands

Telegram handles a small, explicit set: `/start`, `/status`, `/compact`, `/clear`, plus the Telegram-only commands below. Anything else you type with a leading slash — including `/help`, `/save`, and `/load` — is **not** a command here; it falls through to the model as ordinary text. `/exit` and the destructive DangerFullAccess commands are blocked.

Four additional commands are **Telegram-only** (they have no effect in the REPL or TUI):

```
/start               Greeting + what this bot can do.
/voice               Toggle voice output (edge-tts on / off).
/lang he|en          Switch voice transcription + TTS language.
/briefing            Run the morning briefing now (calendar + weather + VIP unread).
```

## Permissions

| Tier | Behaviour | Example tools |
|------|-----------|---------------|
| **ReadOnly** | Auto-allowed | time, note_list, file reads, git status, all external APIs |
| **WorkspaceWrite** | Auto-allowed | note_create, todo_add, web_search, github comment |
| **DangerFullAccess** | Prompts `[y/N]` every time | bash, edit_file, git add/commit/push/checkout, cross-org PRs |

The REPL prompter is interactive. The TUI shows a confirmation modal over the chat — `y` allows; `n`, `Esc`, or `Enter` denies (deny is the default); long inputs scroll with `↑`/`↓` and are never truncated. Telegram bot denies DangerFullAccess by default (no TTY to confirm with).

## Sessions and auto-compaction

- **Autosave** after every REPL turn to `~/.claudette/sessions/last.json`.
- **Resume** with `--resume` or `-r`.
- **Named sessions** via `/save <name>` and `/load <name>` (stored at `~/.claudette/sessions/<name>.json`).
- **Auto-compaction** triggers adaptively at half the active brain's `num_ctx` by default (clamped to `[4000, 1000000]` estimated tokens), so a real 16K–128K window compacts before it overflows — pin an exact trigger via `CLAUDETTE_COMPACT_THRESHOLD=12000`. When it fires, it summarises old turns, keeps recent ones verbatim, and preserves tool-result anchoring.
- **Sliding-window truncator** acts as a safety net inside the API client.
