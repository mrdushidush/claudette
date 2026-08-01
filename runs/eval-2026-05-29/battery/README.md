# Q56 — a hidden-test coding benchmark for local models

**Bench your own model and get a number comparable to the published table.**

Q56 is 56 medium-hard coding tasks run through a real agent loop and graded by **hidden
verification**: the fixture the model sees ships only happy-path tests, and at grade time a
verifier injects hidden reviewer tests and builds/runs them against whatever the model actually
left on disk. Pass/fail is the compiler and the test runner. **There is no LLM judge.**

- **Results:** [`RESULTS-q56.csv`](RESULTS-q56.csv) — 36 full runs, 16 configurations, with each
  run's complete failure list.
- **Method + freeze record:** [`Q50.md`](Q50.md)
- **Campaign analysis:** [`QUALITY-DOSSIER.md`](QUALITY-DOSSIER.md)
- **Publication posture:** [`HELDOUT-SPLIT.md`](HELDOUT-SPLIT.md)

> ### ⚠️ Contamination date: 2026-07-25
> This corpus — fixtures, hidden verifiers **and** reference solutions — has been publicly
> cloneable since 2026-07-25. Every model in the published table was released before that date and
> could not have trained on it. **Any model released after 2026-07-25 must be treated as
> potentially contaminated on these 56 tasks.** Report the date alongside any number you publish.

---

## Requirements

| need | why | notes |
|---|---|---|
| `bash` | the harness is bash | git-bash on Windows is what the published runs used |
| `claudette` on `PATH` | the agent loop under test | `cargo install claudette`, or set `CLAUDETTE_BIN` |
| `cargo` (1.95+) | rust tasks build + `cargo test` | 14 tasks |
| `python` (3.14) + `pytest` | python tasks | 15 tasks |
| `node` (24+) | js tasks, and **ts via native type-stripping** | 19 tasks — older node cannot run the `.ts` tasks |
| an OpenAI-compatible model server | LM Studio, llama-server, anything | default `http://localhost:1234` |

A missing toolchain does not skip its tasks — it **fails** them, which silently costs you up to 19
points. Verify all four before your first run.

## Run it

```bash
cd runs/eval-2026-05-29/battery

BATTERY_MANIFEST=$PWD/manifest-q50.tsv \
BATTERY_TAG=myrun \
CLAUDETTE_MODEL=<your-model-id> \
  bash run_battery.sh

BATTERY_TAG=myrun bash analyze.sh
```

That writes `SCORES-myrun.tsv` (one row per task: id, lang, type, PASS/FAIL, seconds, exit code,
and the verifier's message) and `logs-myrun/` (full transcripts). `analyze.sh` prints the aggregate
plus breakdowns by task type and language.

Point it at a different server with `BATTERY_BASE_URL=http://host:port`. Run a single task or a
prefix with `bash run_battery.sh Q03`.

**Expect 20–80 minutes** for a full pass depending on the model — see the wall-clock column in
`RESULTS-q56.csv`. A VRAM-resident model is roughly 2× faster than a spilled one.

## Hold the constants, and record that you did

Every published row was run at **ctx 32768, KV cache q8_0, one parallel session, full GPU
offload**. If you change any of those, your number is not comparable to the table — the campaign
lost two nights to exactly this.

The trap is that these settings are sticky and mostly invisible. In LM Studio the KV cache type
lives in a per-model JSON that survives unloads, is **not** an `lms load` flag, and is **not**
reported by `lms ps --json`. A freshly downloaded model gets a fresh config in which KV quietly
defaults to f16 and `numParallelSessions` defaults to **4**, splitting your KV allocation four ways
behind a `lms ps` that proudly reports `CONTEXT 32768`.

Two helpers exist for LM Studio users:

```bash
bash preseed_model_config.sh --find <model-substring>   # force the constants BEFORE first load
bash probe_runtime_config.sh <tag>                      # record what was ACTUALLY used
```

`run_battery.sh` calls the probe at the end of a run and appends a row to `RUNMETA.tsv`. On any
other server, record the equivalent settings by hand and publish them with your score.

> **A held-constant nobody measures is not held.**

## Read your number honestly

Four things the published data says about single runs. They will apply to yours too.

1. **The error bar is a property of the model, not the benchmark.** Measured ranges over three
   identical consecutive runs: `gemma-4-e2b` **0**, `devstral-24b` **1**, `qwen3.5-4b` **2**,
   `gpt-oss-20b` **8**. One number is not a result. Run 3×, report the median and the range.
2. **Weakness does not predict variance — the failure mode does.** Models that lose tasks to
   *unparseable output* (bare top-level `return`, literal `\n` in a `.mjs`, unterminated strings)
   swing hard. Models that lose tasks to *missed edge cases* are stable, even when they lose more.
3. **Determinism here is within-session.** `qwen3.5-4b` produced byte-identical failure lists for
   two runs in one sitting, and differed on **ten tasks** from a run on another day. Space at least
   one replicate across sessions, or say that you did not.
4. **Never conclude anything from the identity of a failing task.** Over six runs of one model, 13
   distinct tasks rotated through the failure slots and only one failed every time. Score in
   aggregate.

Also: **the four perf tasks (Q12, Q17, Q32, Q41) grade wall-clock and are contaminated by machine
load.** One of them measured 90 s and 24 s for identical weights and config on a busy vs idle box,
flipping the verdict. Run on an idle machine, or discount those four.

## Add or re-gate a task

Tasks are **test-first gated**. A task may enter the manifest only if its verifier **FAILs** on the
untouched fixture and **PASSes** on fixture + reference solution — which rules out tasks that are
accidentally already passing, and tasks that are impossible.

```bash
bash gate_q50.sh          # re-gate all 56 — expect "56 ok / 0 failing"
bash gate_q50.sh Q03      # one task
```

Layout: `fixtures/<id>/` is what the model sees (copied fresh per run), `verify/<id>.sh` injects
the hidden reviewer tests and grades, `refsol/<id>/` is the reference solution used **only** by the
gate. Rust fixtures each need an empty `[workspace]` table in their `Cargo.toml` or the parent
workspace captures them.

## What this benchmark cannot see

It is a **short-horizon** instrument: iteration depth is p50 = 4, p90 = 8, and only 5 of 56 tasks
reach 9 or more. It grades first-shot code quality. It **cannot** measure context compaction,
prefix-cache behaviour, model escalation, or permission handling — `run_battery.sh` sets
`CLAUDETTE_AUTO_APPROVE=1`, so those paths are never exercised. Proving a compaction fix on Q56
measures net zero.

The top is also saturating: the best configuration scores 55/56. Across all 34 ranking runs, 51 of
56 tasks defeat somebody and only five are never failed by anyone (Q21, Q31, Q33, Q39, Q55).

## Contributing a row

Open a PR or an issue with your `SCORES-<tag>.tsv`, your `RUNMETA` row (or the equivalent
hand-recorded settings), your hardware, and the engine/server version. The three most valuable
contributions right now:

1. **A cross-session replicate** — three runs in one sitting plus a fourth a day later. The
   within-session determinism finding rests on one model.
2. **A second high-variance model** — `gpt-oss-20b` is the only large-range model measured so far.
3. **KV q8 vs f16 on a card where the model is not VRAM-resident** — the published "KV precision
   costs nothing" result may be a resident-model artifact.
