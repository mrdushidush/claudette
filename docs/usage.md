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
| `--tui` | Launch the fullscreen TUI. |
| `--forge "<prompt>"` | Run forge-mode (Planner → Coder → Verifier pipeline) against the active brownfield mission. Requires an active mission — start one with `/brownfield <repo>` or `mission_start` first. |
| `--chat <id>` | Restrict Telegram bot to a specific chat ID. Repeatable, or set `CLAUDETTE_TELEGRAM_CHAT` to a comma-separated list. The bot **default-denies** when no allowlist is provided. |
| `--chat any` | Explicit accept-all: serve every incoming Telegram chat. Required to start the bot with no allowlist. Prints a loud warning. |
| `--auth-google [scope]` | **(integrations)** Run the loopback OAuth flow. Scope is `calendar` (default) or `gmail`. Stores tokens under `~/.claudette/secrets/`. |
| `--revoke` | Pair with `--auth-google` to revoke consent and delete the local token file. |
| `--briefing` | **(integrations)** Write a recurring morning-briefing schedule entry and exit. |
| `--time HH:MM` | Modifier for `--briefing`. Default `07:00`. |
| `--days <spec>` | Modifier for `--briefing`. One of `weekdays` (default), `daily`, or a single weekday name. |
| `--help`, `-h` | Show the flag reference and exit. |
| `--version`, `-V` | Show the Claudette version and exit. |

## Slash commands (REPL + TUI)

```
/help                Show this list.
/validate <path>     Run Codet on an existing code file.
/status              Session info + token counts.
/cost                Lifetime token usage.
/tools               List all tools grouped by capability.
/model               Show the active brain and coder models.
/models              Alias for /model.
/preset fast|auto|smart  Switch model preset.
/brain <model>       Pin the brain model (or "auto" to re-enable fallback).
/coder <model>       Pin the coder model.
/memory              Show CLAUDETTE.MD contents.
/reload              Re-read CLAUDETTE.MD into the system prompt.
/sessions, /ls       List saved sessions.
/save <name>         Save the current session under <name>.
/load <name>         Load a named session.
/compact             Force context compaction now.
/clear               Reset to a fresh session.
/capabilities        Full configuration dump.
/recall <query>      Search past conversations across sessions (semantic).
/undo                Restore the last destructive action (delete/overwrite) from ~/.claudette/trash/.
/brownfield <target> Clone a repo and make it the active mission (one-shot).
/forge <prompt>      Run the forge pipeline against the active mission.
/exit                Leave the REPL.
```

## Telegram-mode slash commands

A subset of the REPL commands works identically inside Telegram chats: `/help`, `/status`, `/compact`, `/clear`, `/save`, `/load`. `/exit` and the destructive DangerFullAccess commands are blocked.

Three additional commands are **Telegram-only** (they have no effect in the REPL or TUI):

```
/voice               Toggle voice output (edge-tts on / off).
/lang he|en          Switch voice transcription + TTS language.
/briefing            Run the morning briefing now (calendar + weather + VIP unread).
```

## Permissions

| Tier | Behaviour | Example tools |
|------|-----------|---------------|
| **ReadOnly** | Auto-allowed | time, note_list, file reads, git status, all external APIs |
| **WorkspaceWrite** | Auto-allowed | note_create, note_update, todo_add, web_search, generate_code, github comment |
| **DangerFullAccess** | Prompts `[y/N]` every time | bash, edit_file, git add/commit/push/checkout, cross-org PRs |

The REPL prompter is interactive. The TUI shows a confirmation modal over the chat — `y` allows; `n`, `Esc`, or `Enter` denies (deny is the default); long inputs scroll with `↑`/`↓` and are never truncated. Telegram bot denies DangerFullAccess by default (no TTY to confirm with).

## Sessions and auto-compaction

- **Autosave** after every REPL turn to `~/.claudette/sessions/last.json`.
- **Resume** with `--resume` or `-r`.
- **Named sessions** via `/save <name>` and `/load <name>` (stored at `~/.claudette/sessions/<name>.json`).
- **Auto-compaction** is effectively off by default (1 M estimated tokens) — opt in for tight context windows via `CLAUDETTE_COMPACT_THRESHOLD=12000`. When it does fire, it summarises old turns, keeps recent ones verbatim, and preserves tool-result anchoring.
- **Sliding-window truncator** acts as a safety net inside the API client.
