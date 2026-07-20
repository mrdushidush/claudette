# Deep-research mode (`--research`)

`claudette --research` points Claudette at the repo you are in and runs an
unattended, hours-capable, **strictly read-only** code review: every file in
2–3-file batches, each batch a fresh conversation, findings checkpointed to
disk after every step, HIGH/MEDIUM findings re-verified, and a final
triage-ready `REPORT.md`.

```sh
cd your-repo
claudette --research                 # review everything
claudette --research error handling  # trailing words = optional focus hint
```

Interrupt it any time (Ctrl-C, reboot, backend crash) — re-running the same
command resumes at the first unfinished batch.

## The guarantees

- **Read-only, enforced at the permission layer — not by prompt.** The
  research runtime is capped at the read-only permission tier: `write_file`,
  `apply_diff`, `bash`, the git-writing tools, and every other mutating tool
  are denied at dispatch no matter what the model asks for. The reviewer
  model never writes a byte; the driver writes all output files itself.
- **Offline, forced.** The run sets `CLAUDETTE_OFFLINE=1` (unless you already
  set it yourself): outbound network is hard-blocked except the local model
  backend. Your code is reviewed by your own GPU; nothing leaves the machine.
- **A bad batch never kills the run.** A batch whose review fails to parse
  twice is recorded as skipped with a note in `FINDINGS.md`, and the run
  moves on.

## How a run works

1. **Manifest.** The driver walks the repo (`.gitignore` respected), plans
   batches of at most 3 files / 48 KB grouped by directory, and writes
   `manifest.json`. Files over 256 KB are skipped with a note.
2. **Batches.** Each batch is reviewed in a *fresh* conversation — no context
   carries over, so a long run cannot drift or spiral. The reviewer must use
   a rigid finding format: severity (`HIGH`/`MEDIUM`/`LOW`/`INFO`), category
   (`bug`, `error-handling`, `security`, `dead-code`, `docs-drift`,
   `test-gap`, `smell`), claim, evidence, and a **mandatory failure
   scenario** — at most 5 findings per batch, and an explicit batch verdict
   even when clean. Findings land in `findings.json` and the human-readable
   `FINDINGS.md`; progress is checkpointed after every batch. Two format
   attempts, then the batch is skipped.
3. **Verify.** Every HIGH/MEDIUM finding is re-examined in its own fresh
   conversation that re-reads the cited file and answers `CONFIRMED` or
   `RETRACTED` (unparseable twice → `UNVERIFIED`). The verify briefing
   rewards retracting weak findings over defending them. LOW/INFO findings
   skip verification.
4. **Synthesize.** One final conversation receives the full findings table
   and writes the report body; the driver writes `REPORT.md` — a generated
   metadata header (target, date, model, coverage, finding counts) above the
   model's ranked, triage-ready report.

## Output files

Everything lands in `~/.claudette/research/<repo>-<date>/` (override with
`CLAUDETTE_RESEARCH_DIR`; the directory must be outside the reviewed repo):

| File | What it is |
|------|------------|
| `manifest.json` | File list + batch plan + content hash (resume safety) |
| `progress.json` | Phase + per-batch state; checkpointed continuously |
| `findings.json` | Structured findings with verdicts (machine-readable) |
| `FINDINGS.md` | Append-only human log, written as batches complete |
| `REPORT.md` | The final ranked report — read this one |

## Resume

Re-run `claudette --research` in the same repo and it picks up at the first
unfinished batch (or the verify/synthesize phase if all batches are done).
If the repo changed since the manifest was built, the hash no longer matches
and the run refuses — delete the output directory or point
`CLAUDETTE_RESEARCH_DIR` somewhere fresh. A completed run (`phase = done`)
also refuses and points you at its `REPORT.md`.

## Knobs

| Env var | Default | Effect |
|---------|---------|--------|
| `CLAUDETTE_RESEARCH_DIR` | `~/.claudette/research/<repo>-<date>/` | Output directory override (used as-is; must be outside the target tree). |
| `CLAUDETTE_RESEARCH_MAX_BATCHES` | unlimited | Stop after N batches — smoke tests / partial runs. A capped run stays in the batches phase; re-running continues it. |
| `CLAUDETTE_RESEARCH_BATCH_FILES` | `3` | Max files per review batch (clamped `1`–`8`). |
| `CLAUDETTE_RESEARCH_EXCLUDE` | unset | Extra paths/dir names to skip, added to the `docs/archive` default. |

## Scoping the review

Gitignored paths (`.gitignore`, `.git/info/exclude`, hidden files) never reach
the manifest. Beyond that, `docs/archive/` is excluded by default — archived
docs are stale by design, and reviewing them just produces `docs-drift` findings
against files nobody maintains. Add your own exclusions with
`CLAUDETTE_RESEARCH_EXCLUDE`:

```sh
CLAUDETTE_RESEARCH_EXCLUDE=vendor,generated,harmony.rs claudette --research
```

An entry with a slash (`docs/archive`, `crates/foo/src/api`) is anchored at the
repo root and matches exactly that path or the subtree beneath it; a bare name
(`vendor`, `harmony.rs`) matches a directory or file of that name at *any* depth
— but not a longer name that merely starts with it (`archive` never touches
`archive.rs`). Excluded files are recorded in
`manifest.json` and counted in the `FINDINGS.md` header, never silently dropped.
To review something that lives under a default exclude, scope the whole run to
that subtree with `CLAUDETTE_WORKSPACE=<subtree>` instead.

Some source files are worth excluding for a different reason: a file dense with
chat-template control tokens (`<|channel|>`, `<|end|>`, …) — for example a
Harmony/Qwen separator-stripping utility that embeds those tokens in its doc
comments and tests — reliably provokes content-less generation when the reviewer
reads it, wasting retries. The run prints a warning naming any such file at
startup; exclude it if the flake cost outweighs the coverage.

## Backend hiccups

A batch whose turns come back content-less gets one immediate retry, then the
driver probes the backend with cheap "reply OK" turns until it generates again —
running `CLAUDETTE_RESEARCH_RECOVER_CMD` once, if set, as a driver-side remedy
(for example `lms unload --all` to force a clean model reload). If the backend
never recovers, the run halts checkpointed; re-invoking resumes at the same
batch. Skips are reserved for batch-bound failures and always record a reason in
`progress.json`. To re-queue previously skipped batches on a resume, set
`CLAUDETTE_RESEARCH_RETRY_SKIPPED=1`.
