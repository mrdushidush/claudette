# Forge-mode

Forge-mode is Claudette's autonomous code-change pipeline. It drives a
Planner → Coder → Verifier → fix-loop → Submitter sequence against an
**active brownfield mission**, ending at `mission_submit` (which auto-
branches, commits, pushes, and opens a PR via `gh_create_pr`).

You start a mission with `/brownfield owner/repo` (or via the
`mission_start` tool) — that clones into `~/.claudette/missions/<slug>/`
and re-routes file operations into the cloned tree. Forge-mode runs **on
top of** that mission state.

> Working on a repo you already have checked out locally? Forge-mode can
> auto-bootstrap an **ephemeral mission** rooted at the cwd's git
> toplevel — no clone, no manual `mission_start`. See "Auto-bootstrap"
> below.

## Worked example

```bash
# 1. Clone the target into a managed mission tree.
claudette
> /brownfield owner/some-repo

# 2. Run the forge pipeline against the current request.
> /forge add a section under "Build" describing the test runner

# 3. Watch the phases roll past in stderr:
#    forge: planner
#    forge: coder (round 0)
#    forge: verifier   score=8 pass=true ...
#    forge: submit

# 4. The Submitter calls mission_submit and the PR opens.
```

From a terminal one-shot:

```bash
claudette --forge "add a section under Build describing the test runner"
```

`--forge` requires a prompt; missing one is a hard error. The trailing
prompt is fed verbatim to the Planner.

## Phases (v0c)

The pipeline lives in [`crates/claudette/src/run.rs`](../crates/claudette/src/run.rs)
inside `run_forge_mission`. Six phases, two of which loop:

1. **Planner** — read-only brain turn (`files` + `search` tools only).
   Localizes the code and emits the relevant file(s) plus a 3–5 step
   numbered plan. The brief is prepended to the Coder's input and shown to
   the Verifier so they inherit the localization. An empty plan is allowed
   (Coder just runs the original prompt).

2. **Coder** — full forge runtime with `files`, `search`, `git`,
   `advanced`, and `github` tool groups pre-enabled and
   `should_submit=false`. The Coder makes the change and **commits it to
   the mission branch** with a clear message, but does **not** push or call
   `mission_submit` — the Verifier reviews first. Each fix-loop round adds
   commits to the same branch; the diff the Verifier grades is
   `base..HEAD`.

3. **Verifier** — two checks, run together each round:
   - A **build + test gate** (on by default): the project's *real* build
     and test suite is run inside the mission tree — `cargo check` +
     `cargo test` on Rust, `go build` + `go test` on Go, `pytest` on
     Python, `npm test` on Node. A build break or a failing test is a
     hard fail; the failures are fed back to the Coder. Infrastructure
     problems (no framework, tool not installed, timeout, "no tests
     collected") stay advisory so a docs PR isn't blocked by a flaky or
     uninstalled suite. Opt out with `CLAUDETTE_FORGE_NO_BUILD_CHECK=1`.
   - A **brain Verifier** turn that scores the captured `git diff` against
     the request and returns one line of JSON:
     ```json
     {"score": 0..=10, "pass": true|false, "feedback": "..."}
     ```
     This gate is **fail-closed**: an unparseable / fenced-only /
     missing-field response abstains as a *fail* (`pass=false, score=0`),
     and a pass requires both `pass=true` **and** `score >= 8`. A stuck
     Verifier therefore exhausts the bounded fix-loop and exits via the
     cap rather than green-lighting unverified code.

4. **Fix-loop** — if the round didn't pass and the pass count is below
   the cap (`DEFAULT_MAX_FIX_ROUNDS = 3` total Coder passes; override with
   `CLAUDETTE_MAX_FIX_ROUNDS`, clamped to 10), the pipeline re-runs Coder
   with the Verifier's feedback prepended. The Coder commits each round to
   the mission branch; the best-scoring round is restored before submit. A
   loop that never passes does **not** open a PR — the commits stay on the
   mission branch for inspection (override with
   `CLAUDETTE_FORGE_SUBMIT_ON_FAIL=1`).

5. **Human-review gate** — before the Submitter runs, forge prints the
   plan + the full final diff and waits for an explicit `y`. This is your
   QA step: anything other than yes — including a non-interactive stdin —
   leaves the commits on the branch and opens no PR. On by default;
   skipped under `CLAUDETTE_FORGE_AUTO_APPROVE=1` (unattended) or
   `CLAUDETTE_FORGE_NO_REVIEW=1`.

6. **Submitter** — final Coder turn with `should_submit=true` that
   **only** calls `mission_submit`. Stage → commit → push →
   `gh_create_pr` happens atomically inside that one tool call.

## Coder / Submitter contract

The Coder phase **commits** its work to the mission branch but must
**not** `git_push` or call `mission_submit` — the Verifier and the
human-review gate run between the commit and the PR. The Verifier grades
the `base..HEAD` diff (the base SHA is snapshotted before any phase runs,
so the diff survives the Coder committing mid-pipeline). An empty diff —
the Coder committed nothing — is forced to a fail so a zero-line PR can't
slip through.

`mission_submit` (the Submitter's only call) auto-branches off
`main`/`master`, then stages → commits any remaining work → pushes →
opens the PR via `gh_create_pr`. It accepts an already-committed, clean
tree (the normal case here): a clean tree with commits ahead of the base
is *submittable*, not "nothing to do".

> If you're writing custom Coder prompts (e.g. a launcher script that
> rephrases `--forge` input), tell the Coder to commit but not push:
> *"Make the change and `git_commit` it to the current branch. Do NOT
> `git_push` or call `mission_submit` — the Verifier reviews first."*

## Auto-bootstrap

When `--forge` / `/forge` runs with no active mission AND the current
working directory is inside a git working tree under `$HOME` (or any
path in `CLAUDETTE_WORKSPACE`), forge-mode auto-creates an **ephemeral
local mission** rooted at the repo's toplevel:

- No clone step (the tree already exists).
- No persisted `.claudette-mission.json` marker (the mission lives in
  memory only, doesn't survive a restart).
- Repo metadata (`mission.repo`) is `None` — `mission_submit` still
  opens a PR if the local clone has a GitHub remote, but it can't
  guess `owner/repo` for arbitrary remotes.

If the forge pipeline errors anywhere between bootstrap and the
Submitter's success, the ephemeral mission is **auto-cleared** so the
next `/forge` call can re-bootstrap from scratch. User-initiated
missions (`/brownfield`, `mission_state(action="attach")`) are left
alone after a forge failure — they're the user's state, not ours.

Out-of-envelope repos (e.g. `/etc/secret-stuff` with no
`CLAUDETTE_WORKSPACE` opt-in) are refused with a clear hint:
*"git repo at /etc/secret-stuff is outside $HOME and CLAUDETTE_WORKSPACE…"*

## `models.toml` role-routing

Forge phases can dispatch different roles to different models. The
overlay lives at `~/.claudettes-forge/models.toml`:

```toml
[planner]
model = "qwen3.5:9b"

[coder]
model = "qwen3-coder:30b"

[verifier]
model = "qwen3.5:9b"
```

The section headers are **lowercase** (`[planner]`, `[coder]`,
`[verifier]`) — the loader matches them case-sensitively and silently
ignores unknown keys, so a capitalized `[Planner]` routes nothing.

| Role | Phase | Default fallback |
|------|-------|------------------|
| `planner` | Planner turn | claudette's active brain |
| `coder` | Coder + fix-loop turns | claudette's active brain |
| `verifier` | Verifier turn | claudette's active brain |

The **Submitter** phase is not separately routable — it runs as the
final Coder turn, so it uses whatever the `coder` role resolves to.

Missing roles silently fall back to claudette's currently-active brain
(the one `current_model()` resolves at runtime — i.e. `CLAUDETTE_MODEL`
or the Auto preset's default). `num_ctx` / `num_predict` are not in
`models.toml`; they carry over from claudette's config so role-routed
turns honour your existing `CLAUDETTE_NUM_CTX` override.

Env-var overrides also exist:

- `CLAUDETTES_FORGE_PLANNER_MODEL`
- `CLAUDETTES_FORGE_CODER_MODEL`
- `CLAUDETTES_FORGE_VERIFIER_MODEL`

Env vars take precedence over `models.toml` when both are set.

The Coder phase also gets a bundled **persona overlay**
(`crates/claudette/personas/codex7.md`) baked into its system prompt
for a consistent code-review/code-write voice. Planner and Verifier
do not currently carry a persona overlay.

## Tuning knobs

| Env var | Default | Effect |
|---------|---------|--------|
| `CLAUDETTE_MAX_FIX_ROUNDS` | `3` | Total Coder passes (round 0 + revisions), clamped to `[1, 10]` |
| `CLAUDETTE_FORGE_NO_REVIEW` | off | Skip the human-review gate (hands-off submit) |
| `CLAUDETTE_FORGE_NO_BUILD_CHECK` | off | Skip the build+test gate |
| `CLAUDETTE_FORGE_TEST_TIMEOUT_SECS` | `180` | Per-step build/test timeout, clamped to `[10, 1800]` |
| `CLAUDETTE_FORGE_AUTO_APPROVE` | off | Unattended: auto-approve tool calls **and** skip the review gate |
| `CLAUDETTE_FORGE_SUBMIT_ON_FAIL` | off | Open a PR for the best revision even if the Verifier never passed |
| `CLAUDETTE_FORGE_SECURITY_REVIEW` | off | Run the deterministic diff security scan as an extra gate |
| `CLAUDETTE_FORGE_SECURITY_OVERRIDE` | off | Submit even if a HIGH-severity security finding survives |
| `CLAUDETTE_FORGE_ALLOW_DIRTY` | off | Allow forge on a dirty/mid-merge tree (ephemeral missions) |

What's still fixed in code:

- The Verifier's grading prompt is fixed in
  [`prompt.rs`](../crates/claudette/src/prompt.rs)
  (`forge_verifier_system_prompt`).
- `mission_submit` is the only PR-opener — direct `gh_create_pr` from
  Submitter is not the intended path.

## Diagnostic checklist

When a forge run misbehaves, run `claudette --doctor` first — it verifies
the model server / brain, the **build toolchains** the Verifier needs
(`git` / `cargo` / `python` / `node` / `go`), and OAuth tokens, with a
copy-paste fix under anything red. Then, on errors specific to forge:

- *"forge: build/test gate FAILED…"* — the change doesn't compile or
  broke a test. The failing commands + errors are in the streamed log and
  fed to the Coder; if it's a false alarm (flaky/networked suite) set
  `CLAUDETTE_FORGE_NO_BUILD_CHECK=1`.
- *"forge: PR not opened — change declined at review."* — you answered
  anything but `y` at the review gate (or stdin wasn't interactive). The
  commits are on the mission branch; re-run `/forge`, or set
  `CLAUDETTE_FORGE_NO_REVIEW=1` to skip the gate.
- *"forge: NOT opening a PR — the Verifier never passed…"* — the fix-loop
  hit its round cap without a pass. Re-run to continue, or set
  `CLAUDETTE_FORGE_SUBMIT_ON_FAIL=1` to open a PR for the best revision.
- *"forge-mode requires an active brownfield mission, and could not
  auto-bootstrap one…"* — you're not in a git repo, or the repo lives
  outside `$HOME` and `CLAUDETTE_WORKSPACE`. `cd` into the repo or set
  `CLAUDETTE_WORKSPACE` to its parent.

## See also

- [`usage.md`](usage.md) — `/forge` and other slash commands
- [`architecture.md`](architecture.md) — how missions, runtimes, and
  brain-selector interact
- [`crates/claudette/src/run.rs`](../crates/claudette/src/run.rs) —
  `run_forge_mission` (orchestration), `build_forge_runtime`,
  `parse_verifier_response`
- [`crates/claudette/src/forge/`](../crates/claudette/src/forge/) —
  `models_toml::ModelMap`, `personas::parse_persona_content`
