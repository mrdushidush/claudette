# 04 — Morning briefing

The ship-line demo for v0.2.0 — Claudette wakes up at 07:00 weekdays
and sends you a summary of today's calendar, current weather, and
anything urgent. Nothing to click; it arrives in Telegram.

## Prerequisites

- Telegram bot set up and receiving messages (see
  [`03-telegram-setup.md`](03-telegram-setup.md)).
- Google Calendar OAuth authorised (see
  [`../docs/google_setup.md`](../docs/google_setup.md)).
- Bot running persistently on a host that stays awake (your laptop
  won't fire briefings while its lid is closed).

## 1. Create the scheduled entry

One command; one shot:

```bash
claudette --briefing
```

That's the default: 07:00 weekdays, canonical briefing prompt, no
voice echo. Output:

```
✨ scheduled briefing 'sch_dhzwrcjigjh0' — every weekday at 07:00
  ▸ next fire: 2026-04-23T07:00:00+03:00
```

The schedule ID is random; the time zone tracks your system local.
Under the hood this is a `catch_up: skip` recurring entry with the
canonical briefing prompt, persisted to `~/.claudette/schedule.jsonl`;
the bot picks it up the next time it starts the scheduler producer.

## 2. Customising time and days

```bash
claudette --briefing --time 08:30 --days weekdays
claudette --briefing --time 06:00 --days daily
claudette --briefing --time 09:00 --days monday
```

Re-running `--briefing` is idempotent — it replaces any existing entry
with the same canonical prompt. Changing the time is a one-liner.

## 3. What it looks like at 07:00

A real briefing (personal details scrubbed):

```
Good morning! Here's your Tuesday, April 22, 2026 briefing:

**Calendar (3 events)**
  - 09:00 — 1:1 with Alex (30m)
  - 14:00 — Design review (1h)
  - 18:00 — Dinner with Dana

**Weather — Tel Aviv**
  22°C now, cloudy with showers expected around 14:00. High 24,
  low 17. Bring a light jacket.

**Unread (top 3 VIPs)**
  - Alice — "Q2 roadmap draft — can you review?"
  - Bob   — "Invoice #4482 needs sign-off"
  - Carol — "Lunch Friday?"

Anything urgent? Say so, I'll break it down.
```

The `--voice` toggle doesn't affect briefings — they're always typed.
(A 07:00 voice announcement in someone else's ear is not a morning
people-pleaser.)

## 4. Catch-up policy

Laptops close. Wi-Fi drops. When the bot starts back up after missing
a briefing:

- `catch_up: skip` (the default for briefings) — silently skip the
  07:00 that was missed. A 07:00 briefing seen at 09:00 is spam.
- `catch_up: once` — fire once on restart.
- `catch_up: all` — fire every missed occurrence (capped at 50 so a
  year-offline bot doesn't spam).

Briefings default to `skip`. Reminders default to `once`.

## 5. Ad-hoc briefings in the chat

You don't need the schedule at all. Inside a Telegram chat:

```
You:    /briefing
Bot:    (same multi-paragraph briefing as above)
```

Useful for testing the pipeline end-to-end before committing to the
07:00 wake-up.

## 6. Under the hood — the three-producer loop

The scheduler is one piece of a single-consumer / two-producer `mpsc`
pattern (see [`../docs/sprint_life_agent.md`](../docs/sprint_life_agent.md)
AD-1). Events go through one channel:

- Telegram poller produces `Event::TgUpdate` (user messages).
- Scheduler produces `Event::Scheduled` (firings due per
  `schedule.jsonl`).
- Consumer thread owns `&mut runtime` and processes one event at a time.

Scheduled firings can't race user messages for session state — both
serialise through the same queue and each runs to completion before
the next starts.

## 7. Listing and cancelling scheduled entries

From inside the REPL or Telegram:

```
> /tools
# confirms 'schedule' group is enabled in Telegram mode

> what do i have scheduled?
  ▸ schedule_list({})
You have 1 entry:
  sch_01H... — every weekday at 07:00 (morning briefing)

> cancel that briefing
  ▸ schedule_cancel({"id": "sch_01H..."})
Cancelled. schedule.jsonl updated.
```

## 8. Gotchas

- **Timezone** — Claudette uses the host's local time. If you move
  timezones, briefings fire at the new-local 07:00 from the next day.
- **Calendar auth expired** — refresh tokens are long-lived but not
  forever. If the briefing says "can't reach Calendar", run
  `claudette --auth-google calendar` again.
- **Persistence** — `schedule.jsonl` uses a write-and-rename atomic
  pattern. Killing the bot mid-write won't corrupt it.
