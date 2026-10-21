# Recording the hero demo GIF

How the GIF at the top of the README is made, so it can be re-shot when the
REPL's output changes. Target: **under 5 MB**, terminal only, no narration,
readable at README column width.

**The take** (`docs/images/claudette-repl-fix.gif`, once recorded) shows one
fix in the REPL: a two-test Python repo where `test_punctuation` fails,
`claudette`, the task typed at the prompt, the colored diff and its `[y/N]`
gate, a `y`, the tests passing through `run_tests`, `/exit`, and
`python -m pytest -q` going green. The scene is the point: Claudette proposes,
a human approves each edit. Until it is recorded,
the README's hero is the screenshot `docs/images/claudette-diff-gate.png`,
which shows the same gate in a real session.

## What you need

- Windows with [vhs](https://github.com/charmbracelet/vhs) 0.12+ and its
  `ttyd`, plus `ffmpeg` (all three install through `winget`). vhs also runs on
  Linux and macOS; change the tape's shell and paths.
- The brain loaded and **warm**: send one throwaway prompt first, so the take
  doesn't sit through a model load.
- The toy repo: `slug.py` with a `slugify()` that lowercases and replaces every
  non-alphanumeric character with `-`, and `test_slug.py` with `test_basic`
  (`"Hello World"` -> `hello-world`, passes) and `test_punctuation`
  (`"Hello, World!"` -> `hello-world`, fails on the doubled dash). Commit it
  all, so a retake is `git checkout -- .`.
- The `claudette` binary you want to show, first on the tape's `PATH`.

## The tape

[`hero-demo.tape`](hero-demo.tape) holds the whole take. Its hidden first line
sets `PATH`, the backend env vars, `CLAUDETTE_NO_SPINNER=1` and a short `> `
prompt; then it:

1. runs `python -m pytest -q` (1 failed, 1 passed),
2. starts `claudette` and waits for the `›` prompt,
3. types the task, ending with "Check with run_tests." The REPL runs in
   workspace-write mode, so `run_tests` passes without a prompt and the edit
   is the only `[y/N]` in the take,
4. waits for `Allow?` on the cursor's line, pauses on the diff, and types `y`.
   **No Enter**: the gate reads a single key, and an Enter would answer the
   next prompt,
5. waits for the `›` prompt again, types `/exit`, and runs pytest (2 passed).

Three things that cost retakes. The spinner must be off: with it on, every
frame differs while the model thinks, and the encoder can't trim that stretch
as idle. Every `Wait` is `+Line` (the cursor's line), never `+Screen`, or a
prompt that already scrolled by satisfies a later wait. And if the model asks
a second time anyway (a second edit, a `bash` call), the wait for `›` times
out and the take fails: reset the repo and re-run.

```powershell
cd D:\demo                         # the tape's paths assume D:\demo\slugs and D:\demo\bin
git -C slugs checkout -- .         # clean repo
Remove-Item -Recurse out -ErrorAction Ignore
vhs hero-demo.tape                 # writes PNG frames to out\frames\
python make-hero-gif.py out\frames claudette-repl-fix.gif
```

## Frames to GIF

[`make-hero-gif.py`](make-hero-gif.py) merges runs of identical frames and caps
every still stretch at 1.6 s, so the model's thinking time doesn't become dead
air, holds the last frame for 3 s, and encodes with an ffmpeg palette. It
prints the length before and after and the file size. Over 5 MB: lower
`--max-idle`, or shorten the take.

## Ship it

```sh
cp claudette-repl-fix.gif docs/images/claudette-repl-fix.gif
```

The README's first image points at that file. Update its alt text and the
caption under it if the scene changes.
