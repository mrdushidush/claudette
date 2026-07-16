# Recording the hero demo GIF (`docs/images/forge-demo.gif`)

Shot list + exact commands for the 30–45 second recording that replaces the
static PNG at the top of the README. Target: **< 5 MB**, terminal-only, no
narration, readable at README column width.

## Setup (before recording)

- Linux/macOS terminal (asciinema doesn't record on Windows; use WSL or the
  Linux box). 100×30 terminal, a font that renders `↳`/`⚠`/box-drawing glyphs,
  dark theme, font size large enough that a phone reader can follow.
- Brain loaded and **warm** (run one throwaway prompt first — the demo must not
  eat a 60 s model-load hang).
- A small toy repo with a test suite (ideal: a tiny Rust or Python CLI of
  ~5 files where `--forge` can add a small flag). Clean working tree — forge
  refuses a dirty tree.
- `export CLAUDETTE_NO_SPINNER=` (leave the spinner ON — it reads as "alive"
  in the GIF).
- Install the recorder + converter:

```sh
cargo install --locked agg          # asciinema GIF renderer
# asciinema via your package manager (apt install asciinema / brew install asciinema)
```

## Shot list (one continuous take, ~40 s)

| # | Seconds | Action | What the viewer must see |
|---|---------|--------|--------------------------|
| 1 | 0–6 | `claudette --doctor` | The green rows — local brain, toolchains — and the GPU model recommendation. **All green.** |
| 2 | 6–14 | `claudette "add a note: demo day"`, then `claudette "list my notes"` | One-shot round trip: instant tool call, instant answer. Proves it's alive and local. |
| 3 | 14–38 | `claudette --forge "<small task>"` in the toy repo | The phase lines streaming: `forge: planner` → `forge: coder (round 0)` → `forge: build + test` → `forge: verifier … pass=true`. |
| 4 | 38–44 | The review gate | The **colored diff** and the `⚠ … [y/N]` prompt — pause 2 s on it, type `y` (GitHub-mission take) or let the local-mission commit line land. **This is the money shot: a human gate in an autonomous pipeline.** |

Keep-it-tight rules: no typos/backspacing (rehearse the take), no window
switching, don't scroll back. If the forge round takes longer than ~25 s,
cut the take and pick a smaller task — a boring wait kills the GIF.

## Record → convert → verify

```sh
asciinema rec demo.cast --cols 100 --rows 30      # run the shot list, Ctrl+D to stop
agg demo.cast forge-demo.gif \
    --font-size 16 --speed 1.25 --theme monokai   # 1.25× trims dead air
ls -lh forge-demo.gif                             # must be < 5 MB
```

Too big? In order of preference: `--speed 1.5`, re-record with fewer idle
seconds, `--font-size 14`, or post-process with
`gifsicle -O3 --lossy=80 forge-demo.gif -o forge-demo.gif`.

## Ship it

```sh
mv forge-demo.gif docs/images/forge-demo.gif
```

Then in `README.md`, replace the `claudette-ships-pr.png` image line (it has a
`TODO(onboarding 1.3)` comment above it) with:

```markdown
![claudette --doctor going green, a one-shot answer, then Forge planning, coding, passing the build-and-test gate, and stopping at the human diff review](docs/images/forge-demo.gif)
```

Keep the PNG in `docs/images/` — it stays as the social-preview / fallback
image.
