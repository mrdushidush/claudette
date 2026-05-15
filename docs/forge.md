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
inside `run_forge_mission`. Five phases, two of which loop:

1. **Planner** — tool-less brain turn. Decomposes the request into a
   3–5 step numbered plan. Output is prepended to the Coder's input. An
   empty plan is allowed (Coder just runs the original prompt).

2. **Coder** — full forge runtime with `files`, `search`, `git`,
   `advanced`, and `github` tool groups pre-enabled and
   `should_submit=false`. The Coder edits the working tree but
   **must not commit** — see "Submitter contract" below.

3. **Verifier** — tool-less brain turn that scores the captured
   `git diff HEAD` against the original request. Returns a JSON object:
   ```json
   {"score": 0..=10, "pass": true|false, "feedback": "..."}
   ```
   Unparseable responses (broken JSON, prose-only output, etc.) fall
   through to a permissive `pass=true, score=10, feedback=""` default so
   a flaky local Verifier model can't deadlock a working Coder.

4. **Fix-loop** — if Verifier `pass=false` and the round count is below
   `MAX_FIX_ROUNDS` (currently `2`, see
   [`run.rs:286`](../crates/claudette/src/run.rs)), the pipeline re-runs
   Coder with the Verifier's feedback prepended. The Coder edits the
   *same* tree; commits are still deferred to Submitter. After
   `MAX_FIX_ROUNDS` failed rounds the pipeline submits anyway with the
   final Verifier message logged — better to ship a flawed PR the human
   can review than burn the whole context budget chasing a stuck
   Verifier.

5. **Submitter** — final Coder turn with `should_submit=true` that
   **only** calls `mission_submit`. Stage → commit → push →
   `gh_create_pr` happens atomically inside that one tool call.

## Submitter contract

The Coder phase must **leave the working tree dirty**: modified files,
no commit. `mission_submit` refuses with "No changes detected in the
working tree. Mission cannot be submitted..." if the tree is clean, so
a Coder that runs `git_add` + `git_commit` before exiting will produce
a successful runtime trace with **no PR opened** — exactly the silent
failure mode that surfaced on 2026-05-15 from a forge_e2e prompt asking
Coder to commit early.

> If you're writing custom Coder prompts (e.g. a launcher script that
> rephrases `--forge` input), include the explicit phrase:
> *"Do NOT call git_add, git_commit, git_push, or mission_submit. The
> Submitter phase will stage + commit + push + open the PR for you."*

The Verifier also relies on this: it reads `git diff HEAD` of the dirty
tree to score the change. A clean tree gives it nothing to verify.

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
missions (`/brownfield`, `mission_attach`) are left alone after a
forge failure — they're the user's state, not ours.

Out-of-envelope repos (e.g. `/etc/secret-stuff` with no
`CLAUDETTE_WORKSPACE` opt-in) are refused with a clear hint:
*"git repo at /etc/secret-stuff is outside $HOME and CLAUDETTE_WORKSPACE…"*

## `models.toml` role-routing

Forge phases can dispatch different roles to different models. The
overlay lives at `~/.claudettes-forge/models.toml`:

```toml
[Planner]
model = "qwen3.5:9b"

[Coder]
model = "qwen3-coder:30b"

[Verifier]
model = "qwen3.5:9b"

[Submitter]
model = "qwen3.5:4b"
```

| Role | Phase | Default fallback |
|------|-------|------------------|
| `Planner` | Planner turn | claudette's active brain |
| `Coder` | Coder + fix-loop turns | claudette's active brain |
| `Verifier` | Verifier turn | claudette's active brain |
| `Submitter` | Final `mission_submit` turn | claudette's active brain |

Missing roles silently fall back to claudette's currently-active brain
(the one `current_model()` resolves at runtime — i.e. `CLAUDETTE_MODEL`
or the Auto preset's default). `num_ctx` / `num_predict` are not in
`models.toml`; they carry over from claudette's config so role-routed
turns honour your existing `CLAUDETTE_NUM_CTX` override.

Env-var overrides also exist:

- `CLAUDETTES_FORGE_PLANNER_MODEL`
- `CLAUDETTES_FORGE_CODER_MODEL`
- `CLAUDETTES_FORGE_VERIFIER_MODEL`
- `CLAUDETTES_FORGE_SUBMITTER_MODEL`

Env vars take precedence over `models.toml` when both are set.

The Coder phase also gets a bundled **persona overlay**
(`crates/claudette/personas/codex7.md`) baked into its system prompt
for a consistent code-review/code-write voice. Planner and Verifier
do not currently carry a persona overlay.

## Things you can't change yet

- The fix-loop budget is a const (`MAX_FIX_ROUNDS = 2`). Tuning is a
  follow-up.
- The Verifier's grading prompt is fixed in
  [`prompt.rs`](../crates/claudette/src/prompt.rs)
  (`forge_verifier_system_prompt`).
- `mission_submit` is the only PR-opener — direct `gh_create_pr` from
  Submitter is not the intended path.

## Diagnostic checklist

When a forge run misbehaves, run `claudette --doctor` first — it
verifies Ollama / brain / OAuth tokens. Then, on errors specific to
forge:

- *"No changes detected in the working tree."* — the Coder committed
  early. See "Submitter contract" above.
- *"forge-mode requires an active brownfield mission, and could not
  auto-bootstrap one…"* — you're not in a git repo, or the repo lives
  outside `$HOME` and `CLAUDETTE_WORKSPACE`. `cd` into the repo or set
  `CLAUDETTE_WORKSPACE` to its parent.
- *"verifier still failing after 2 round(s); submitting anyway"* —
  Coder couldn't satisfy Verifier in two rounds. The PR was opened
  anyway with the final feedback in the streamed log — review it
  manually before merging.

## See also

- [`usage.md`](usage.md) — `/forge` and other slash commands
- [`architecture.md`](architecture.md) — how missions, runtimes, and
  brain-selector interact
- [`crates/claudette/src/run.rs`](../crates/claudette/src/run.rs) —
  `run_forge_mission` (orchestration), `build_forge_runtime`,
  `parse_verifier_response`
- [`crates/claudette/src/forge/`](../crates/claudette/src/forge/) —
  `models_toml::ModelMap`, `personas::parse_persona_content`
