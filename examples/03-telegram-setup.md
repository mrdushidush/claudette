# 03 — Telegram bot setup

How to get Claudette running as a Telegram bot end-to-end. Counts as
one of the most impactful interfaces — you get remote access to your
local agent with voice in and voice out.

## Prerequisites

- Claudette built and on `PATH` (see main [`README.md`](../README.md)).
- Ollama running locally with your chosen models pulled.
- A Telegram account.

## 1. Create a bot

Message [@BotFather](https://t.me/BotFather) on Telegram:

```
/newbot
<your-bot-name>             (whatever you like, shown in chat)
<your-bot-username>_bot     (must end with 'bot' — Telegram rule)
```

BotFather replies with a token that looks like
`1234567890:ABC-DEF...`. Keep that string — you won't be able to
recover it later without revoking and regenerating.

## 2. Give Claudette the token

Two options; env var wins if both are set.

**Option A — env var** (recommended for shell users):

```bash
export TELEGRAM_BOT_TOKEN="1234567890:ABC-DEF..."
```

Add it to `~/.claudette/.env` so it persists across shells:

```
TELEGRAM_BOT_TOKEN=1234567890:ABC-DEF...
```

**Option B — secret file:**

```bash
mkdir -p ~/.claudette/secrets
echo "1234567890:ABC-DEF..." > ~/.claudette/secrets/telegram.token
chmod 600 ~/.claudette/secrets/telegram.token
```

## 3. Start the bot

```bash
claudette --telegram --chat 123456789
```

Claudette prints:

```
🤖 telegram bot mode @your-bot-username_bot
✨ serving chat IDs: [123456789]
✨ voice transcription ready (ffmpeg + whisper)
✨ voice output ready (edge-tts)
```

Replace `123456789` with your own Telegram chat ID (see §5 below for
how to find it). A bare `claudette --telegram` with no allowlist exits
immediately with a "refusing to start: no chat allowlist" error — this
is the shipped default-deny posture. To explicitly accept every
incoming chat instead, pass `--chat any` (the bot prints a loud
warning on startup).

## 4. First message

Open Telegram, find your bot by username, send `/start`. Claudette
replies in the chat. On the first incoming message Claudette remembers
the chat ID at `~/.claudette/secrets/telegram_chat.id` (one ID per
line) so scheduled briefings know where to send.

Example exchange:

```
You:    What's on my calendar this week?
Bot:    Claudette is typing...
Bot:    You have 3 events scheduled this week:
          - Mon 09:00 — 1:1 with the team
          - Wed 14:00 — Project review
          - Fri 18:00 — Dinner with Dana
        Nothing urgent tomorrow.
```

(This assumes you've already authorised the Calendar scope — see
[`../docs/google_setup.md`](../docs/google_setup.md).)

## 5. Restricting to specific chats

Claudette's Telegram bot **default-denies.** Starting `claudette
--telegram` with no `--chat <id>` allowlist and no
`CLAUDETTE_TELEGRAM_CHAT` env var exits immediately with an error —
prevents the "I ran the bot to test it and now anyone who guesses the
username gets a full assistant" footgun.

The allowlist:

```bash
claudette --telegram --chat 123456789 --chat 987654321
```

Or set the `CLAUDETTE_TELEGRAM_CHAT` env var to a comma-separated
list of IDs. Either way, messages from chats not in the list are
silently dropped.

### Finding your own chat ID

Send `/start` to the bot once from your account. The bot's logs print
the incoming chat's ID and name; copy that into `--chat <id>`.

Alternatively, inside the REPL:

```
> enable the telegram group, then poll for updates and tell me what chat IDs you see
  ▸ enable_tools({"group": "telegram"})
  ▸ tg_get_updates({})
I see one chat ID: 123456789 (Alice).
```

### Opt-in accept-all mode

If you really want the bot to serve every incoming chat (for a public
support-bot use case, say):

```bash
claudette --telegram --chat any
```

The bot prints a loud warning on startup and runs with no allowlist.
`--chat any` in this mode does NOT permanently persist incoming
strangers to the trust set — it's a per-run flag, not a one-way door.

## 6. Voice

Voice input (speech-to-text) and voice output (TTS) are opt-in.

**Input** — Whisper transcribes voice messages. Install
[whisper.cpp](https://github.com/ggerganov/whisper.cpp), download the
`ggml-large-v3-turbo.bin` model to `~/.claudette/models/`. Send a voice
message; Claudette transcribes and handles it like text.

**Output** — [edge-tts](https://github.com/rany2/edge-tts) reads
replies back. Toggle with `/voice` in the chat. Language with
`/lang he` or `/lang en`.

Voice output is gated on `input_was_voice` — typed questions stay
typed even with TTS on. Voice output is also suppressed during the
morning briefing (you don't want your phone loudly announcing email
previews).

## 7. Slash commands inside Telegram

A subset of the REPL slashes work identically in Telegram:
`/help`, `/status`, `/compact`, `/clear`, `/save`, `/load`.
Destructive ones (`/exit`, bash, edit_file, git commit/push) are
blocked — no interactive TTY to confirm.

Two commands are Telegram-only:

- `/voice` — toggle voice output on/off.
- `/lang he|en` — switch transcription + TTS language.
- `/briefing` — on-demand morning briefing (see
  [`04-morning-briefing.md`](04-morning-briefing.md)).

## 8. Running headless

For a persistent deployment: a small systemd unit or a `tmux`
session on a home server is enough. Claudette is single-threaded,
single-binary — no ceremony, no Docker. The Telegram bot polls
`getUpdates` every 2s; CPU usage when idle is effectively zero.

## Troubleshooting

**"Telegram bot token not found"** — env var not set and
`~/.claudette/secrets/telegram.token` is missing or empty.

**"Conflict: terminated by other getUpdates request"** — you have
Claudette running in two places against the same bot token. Kill one.

**Bot replies but commands don't work** — some Telegram clients pad
commands with the bot username (`/help@mybot`). Claudette strips that.
If you still see issues, report at
<https://github.com/mrdushidush/claudette/issues>.
