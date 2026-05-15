# Quickstart

Five minutes from zero to a first conversation, then a tour of the common flows.

## Install

```bash
# 1. Install Ollama from https://ollama.com (one-time).
ollama serve &

# 2. Pull the default brain.
ollama pull qwen3.5:4b

# 3. Install Claudette.
cargo install claudette

# 4. Smoke-test.
claudette "what time is it?"
```

That's it for the base install. The `qwen3.5:4b` brain is ~3.4 GB on disk, ~3.4 GB VRAM, and handles all of Claudette's tool-using flows on its own.

## Four ways to talk to Claudette

```bash
claudette                            # interactive REPL
claudette --tui                      # fullscreen TUI (5 tabs)
claudette "your prompt here"         # one-shot, prints reply and exits
claudette --telegram                 # Telegram bot mode
claudette --resume                   # resume last session
```

Each mode runs the same conversation runtime, the same tool set, and the same session format. Switching is just a different entry point.

## First flows to try

### Notes and todos (no tokens needed)

```
> take a note: pick up bread tomorrow
> what's on my todo list?
> add a todo: review PR #42
> mark "review PR #42" as done
```

### Time, weather, Wikipedia (no tokens needed)

```
> what time is it?
> what's the weather in Tokyo?
> summarise the Wikipedia article on the Ariane 6 rocket
```

### Web search (needs `BRAVE_API_KEY`)

Get a key from [api.search.brave.com](https://api.search.brave.com/). Then:

```bash
export BRAVE_API_KEY=your_key
claudette
> search the web for "rust async runtime benchmarks 2026"
```

### GitHub workflows (needs `GITHUB_TOKEN`)

```bash
export GITHUB_TOKEN=ghp_...
claudette
> list open issues on mrdushidush/claudette
> what's the status of PR #5?
```

### Brownfield missions: clone, edit, ship a PR

```
> /brownfield owner/some-repo
> read README.md and find the place that documents the build
> add a section under "Build" describing the test runner
> /forge open the PR
```

`/brownfield` clones into `~/.claudette/missions/<slug>/` and silently re-routes file operations into the mission tree. `/forge` runs a Planner→Coder→Verifier pipeline that ends at `mission_submit`, which auto-branches, commits, pushes, and opens the PR. If you're already cd'd into a git repo, `claudette --forge "<prompt>"` auto-bootstraps an ephemeral mission rooted at the repo toplevel — no `/brownfield` step needed. Full pipeline walkthrough: [forge.md](forge.md).

### Google Calendar + Gmail (needs OAuth)

Walkthrough: [google_setup.md](google_setup.md).

```bash
claudette --auth-google calendar
claudette --auth-google gmail
claudette
> what's on my calendar tomorrow?
> any unread email from VIP senders?
```

### Telegram bot with voice

Get a token from `@BotFather`. Set `TELEGRAM_BOT_TOKEN`. Pull a Whisper model under `~/.claudette/models/ggml-large-v3-turbo.bin`.

```bash
claudette --telegram --chat any   # accept all chats; for production set --chat <id>
```

Send a voice note. Claudette transcribes it (Whisper), runs the turn, replies in text. Type `/voice` to also get spoken replies via edge-tts.

### Morning briefing

Persistent scheduler entry that fires at 07:00 weekdays and prints calendar + weather + unread email:

```bash
claudette --briefing                       # default: 07:00 weekdays
claudette --briefing --time 08:30 --days weekdays
```

## Where to go next

- [`configuration.md`](configuration.md) — every env var, token file fallbacks, recall settings
- [`hardware.md`](hardware.md) — what VRAM you need at each preset, 30b-on-8GB recipe
- [`usage.md`](usage.md) — full CLI flag reference + every slash command
- [`architecture.md`](architecture.md) — module layout, tool-group contract, Codet sidecar
- [`comparison.md`](comparison.md) — honest side-by-side vs. other open-source agents
