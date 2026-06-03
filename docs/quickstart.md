# Quickstart

Zero to a working agent in **under five minutes**, then a 60-second tour of the
TUI and the Forge code-change pipeline.

## 1. Install (2 min)

```bash
# a. Install Ollama from https://ollama.com (one-time).
ollama serve &

# b. Pull the default brain (~3.4 GB on disk, ~3.4 GB VRAM).
ollama pull qwen3.5:4b

# c. Install Claudette.
cargo install claudette
```

The `qwen3.5:4b` brain handles every tool-using flow on its own. Prefer LM
Studio or a bigger model? See [configuration.md](configuration.md) and
[hardware.md](hardware.md).

## 2. Verify (30 sec)

```bash
claudette --doctor
```

`--doctor` probes every dependency and prints a green/red report with a
**copy-paste `↳ fix:` command** under anything that's broken — model server not
running, brain not pulled, a missing build toolchain (`git` / `cargo` /
`python` / `node` / `go`), or absent voice deps. Green "local brain" and "build
toolchains" rows mean you're ready. Run it any time something misbehaves.

## 3. First conversation (30 sec)

```bash
claudette "what time is it?"
```

Four entry points, one runtime, one session format — switching is just a
different command:

```bash
claudette                            # interactive REPL
claudette --tui                      # fullscreen TUI
claudette "your prompt here"         # one-shot, prints reply and exits
claudette --telegram --chat any      # Telegram bot
claudette --resume                   # resume last session
claudette --offline "..."            # enforced air-gap: block all cloud egress
```

Add `--offline` (or set `CLAUDETTE_OFFLINE=1`) to any of these to **enforce**
the air-gap — every outbound call except the local model backend + loopback is
hard-blocked, so the brain and recall keep working but web search, mail, GitHub,
Telegram, and remote git all refuse. `claudette --offline --doctor` prints the
exact allow-list. See [Air-gapped by design](../README.md#-air-gapped-by-design).

## The TUI in 60 seconds

```bash
claudette --tui
```

Five tabs across the top — switch with number keys or cycle with `Tab` /
`Shift+Tab`:

| Key | Tab | What's there |
|-----|-----|--------------|
| `1` | **Chat** | Streaming conversation + inline tool calls |
| `2` | **Tools** | Full tool-event log (every call, args, result) |
| `3` | **Notes** | Browse `~/.claudette/notes/` — `↑`/`↓` select, `f` filter by tag |
| `4` | **Todos** | `↑`/`↓` select, `Space`/`Enter` toggle done |
| `5` | **HW** | Live GPU / VRAM / temperature |

- **Type and press `Enter`** to send (number keys only switch tabs when the
  input box is empty, so you can still type "1pm").
- **Slash commands work here too** — `/help`, `/brownfield`, `/forge`, `/recall`,
  everything the REPL has.
- **`Ctrl+V`** pastes an image or text block into your next message.
- **`Ctrl+C`** (or `Ctrl+D`) quits. (`Ctrl+G` is a coffee break — try it.)

## Forge: hands-off code changes with a review gate

Forge runs an autonomous **Planner → Coder → Verifier → fix-loop → Submitter**
pipeline against a git repo. It builds, tests, and ends by opening a PR — and it
asks for your sign-off before the PR goes out.

### Against a GitHub repo (review gate → PR)

```
> /brownfield owner/some-repo
> /forge make the --timeout flag accept fractional seconds
```

`/brownfield` clones into `~/.claudette/missions/<slug>/` and re-routes file ops
into the clone. `/forge` runs the pipeline; watch the phases stream past:

```text
forge: planner                 # localizes the code, writes a short plan
forge: coder (round 0)         # makes the edit, commits to the mission branch
forge: build + test            # cargo check / cargo test (py/js/go too)
forge: verifier   score=9 pass=true
forge: review — approve before opening the PR
  ── plan ──
  ...
  ── diff ──
  ...
  ⚠ Open the PR with these changes? [y/N]
```

Two things make this trustworthy:

1. **The Verifier actually builds and tests.** Each round it runs the project's
   real build + test suite in the tree (`cargo check`/`cargo test`,
   `go build`/`go test`, `pytest`, `npm test`). A diff that doesn't compile or
   breaks a test can't pass — the failures are fed back to the Coder to fix.
2. **You QA before the PR ships.** The review gate prints the plan + the full
   final diff and waits for an explicit `y`. Anything else (including a piped,
   non-interactive stdin) leaves the commits on the mission branch and opens no
   PR — re-run `/forge` to continue, or push manually.

### Against a local repo (commits to a branch, no PR)

```bash
cd ~/code/your-project          # any git repo under $HOME
claudette --forge "make the --timeout flag accept fractional seconds"
```

Inside an existing repo with no active mission, Forge auto-bootstraps an
ephemeral mission at the repo root — no clone, no setup. It runs the same
build-and-test-verified pipeline, then **commits the result to an isolated
`claudette-mission/*` branch** and restores your working branch untouched. There
is no PR (and so no review gate) for a local mission — review the branch with
`git log` / `git diff`, then `git merge` or `git branch -D` as you see fit.

Useful knobs (all optional):

| Env var | Effect |
|---------|--------|
| `CLAUDETTE_FORGE_NO_REVIEW=1` | Skip the human-review gate (fully hands-off PR) |
| `CLAUDETTE_FORGE_NO_BUILD_CHECK=1` | Skip the build+test gate (slow/networked suites) |
| `CLAUDETTE_FORGE_TEST_TIMEOUT_SECS=300` | Per-step build/test timeout (default 180) |
| `CLAUDETTE_FORGE_AUTO_APPROVE=1` | Unattended: auto-approve tool calls **and** skip the review gate |

Full pipeline walkthrough + role-routing: [forge.md](forge.md).

## More first flows

### Notes, todos, time, weather, Wikipedia (no tokens needed)

```
> take a note: pick up bread tomorrow
> add a todo: review PR #42
> what's the weather in Tokyo?
> summarise the Wikipedia article on the Ariane 6 rocket
```

### Web search (needs `BRAVE_API_KEY`)

Get a key from [api.search.brave.com](https://api.search.brave.com/):

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

### Google Calendar + Gmail (needs OAuth)

Walkthrough: [google_setup.md](google_setup.md).

```bash
claudette --auth-google calendar
claudette --auth-google gmail
claudette
> what's on my calendar tomorrow?
```

### Telegram bot with voice

Get a token from `@BotFather`, set `TELEGRAM_BOT_TOKEN`, pull a Whisper model
under `~/.claudette/models/ggml-large-v3-turbo.bin`:

```bash
claudette --telegram --chat any   # accept all chats; for production use --chat <id>
```

Send a voice note — Claudette transcribes it (Whisper), runs the turn, replies
in text. Type `/voice` for spoken replies too.

### Morning briefing

```bash
claudette --briefing                       # 07:00 weekdays: calendar + weather + email
claudette --briefing --time 08:30 --days weekdays
```

## Where to go next

- [`configuration.md`](configuration.md) — every env var, token fallbacks, recall settings
- [`forge.md`](forge.md) — the full Forge pipeline, review gate, build/test gate, role-routing
- [`hardware.md`](hardware.md) — VRAM per preset, the 30b-on-8GB recipe
- [`usage.md`](usage.md) — full CLI flag reference + every slash command
- [`architecture.md`](architecture.md) — module layout, tool-group contract, Codet sidecar
- [`comparison.md`](comparison.md) — honest side-by-side vs. other open-source agents
