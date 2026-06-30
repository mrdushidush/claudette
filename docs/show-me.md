# Show me — what can I actually ask Claudette?

You don't need to learn a command syntax. Open Claudette (REPL, TUI, or Telegram) and type or speak in plain English. These are real prompts that work today.

If you haven't installed yet, see the [one-line installer in the README](../README.md#install).

![Claudette TUI](images/claudette-tui.png)

---

## Notes and todos

Claudette keeps a markdown notebook and a task list under `~/.claudette/`. Nothing is sent anywhere.

- *"Remember that the storage closet key lives behind the printer."*
- *"Add 'pay the gas bill before Friday' to my todos."*
- *"What did I write down about the dishwasher last month?"*
- *"List all my open todos."*
- *"Mark the gas bill task done."*

You'll get the note back from a search later even if you forgot the exact words you used — Claudette searches by meaning, not just keywords.

---

## Calendar and email

Once you've connected Google (one-time OAuth — see [`google_setup.md`](google_setup.md)), Claudette can manage your calendar and read your inbox.

- *"What's on my calendar tomorrow?"*
- *"Schedule a 30-minute coffee with Sam on Thursday at 3pm."*
- *"Move my Tuesday dentist appointment to Wednesday at the same time."*
- *"Did Lisa email me about the lease this week?"*
- *"Summarize the unread mail from my landlord."*

Calendar events are created, moved, and cancelled only after you confirm. Your **inbox is read-only** — Claudette can search and read mail (sending and drafting aren't built yet), so nothing ever leaves on its own.

---

## Weather, news, knowledge

These work out of the box; some surface more detail with a free API key (Brave, etc — see [`configuration.md`](configuration.md)).

- *"Is it going to rain in Tel Aviv tonight?"*
- *"Look up the half-life of cesium-137."*
- *"Who won the 1994 World Cup, and what was the score?"*
- *"Find me three news articles from this week about Mars Sample Return."*

---

## Drop in a screenshot

Press <kbd>Alt</kbd>+<kbd>V</kbd> in the TUI (paste from clipboard), drag an image in, or type `@/path/to/image.png`. Then ask whatever you want about it. Works with any multimodal model you've pulled into Ollama.

- *"What does this error message mean?"* (after pasting a screenshot of a crash)
- *"Critique this landing page mockup — what would a first-time visitor miss?"*
- *"Read the text on this receipt and total the line items by category."*
- *"Is this resume layout typographically balanced?"*

---

## Voice from your phone

Run `claudette --telegram` on your home PC. Open Telegram on your phone. Send Claudette a voice note from anywhere — it transcribes (Whisper), answers, and talks back (edge-tts) in English or Hebrew.

- *"Add tomatoes and basil to the shopping list."* (walking through the supermarket)
- *"What time does the post office close on Friday?"*
- *"Tell me Friday's calendar."*
- *"Take a note: the boiler is making a thumping noise at startup."*

Telegram default-denies anything destructive (no shell, no git push from your phone) because there's no TTY to confirm.

---

## Memory across sessions

`/recall <topic>` searches every past conversation across every session by meaning, and drops the relevant snippets into the current context.

- *"/recall what I tried last week about the slow database query"*
- *"/recall the article I summarized about ADHD medication dosing"*
- *"/recall my notes on the Berlin trip"*

If you ever told Claudette something in any session, you can pull it back.

---

## Code

Claudette is first and foremost a coding agent. `generate_code` and `--forge` route through a dedicated coder model.

- *"Write me a Python script that renames every .HEIC in this folder to .jpg using ffmpeg."*
- *"Add a dark-mode toggle to my React app's settings page."*
- *"Refactor this Rust function to use iterators instead of indexed loops."*
- *"/forge add a --json flag to my CLI's export command that emits NDJSON"*

`--forge` runs a Planner → Coder → Verifier loop autonomously, opens a PR when it converges, and re-tries with feedback up to a configurable round limit. See [`forge.md`](forge.md).

---

## Briefings, schedules, reminders

- *"Wake me with a briefing every weekday at 7:30 — calendar, top three news headlines, weather."*
- *"Remind me to call mom every Sunday at 6pm."*
- *"Every morning at 8, check if anything's expiring on my Google Drive and tell me."*

Schedules persist across restarts.

---

## What if I get stuck?

- `/help` — list every slash command.
- `claudette --doctor` — diagnose Ollama, models, tokens, permissions.
- Plain English works: *"what can you do?"*, *"how do I enable Google calendar?"*, *"why isn't the weather tool working?"*

Open an issue at <https://github.com/mrdushidush/claudette/issues> if something genuinely doesn't behave.
