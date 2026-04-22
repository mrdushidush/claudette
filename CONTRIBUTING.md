# Contributing to Claudette

Thanks for taking an interest. Claudette is a solo-maintainer project;
contributions are welcome but reviewed as time allows — please be
patient, and don't treat a delayed response as disinterest.

## Before you start

**File an issue first for anything non-trivial.** A two-line issue
saves us both from a 500-line PR that doesn't fit the project's
direction. Bug reports with a reproducer are always welcome without
prior discussion; feature proposals work best as an issue first.

**What Claudette is:** a local-first AI personal secretary in Rust.
Single binary, Ollama-only brain path by default, four interfaces
(REPL, one-shot, TUI, Telegram bot). See
[`docs/comparison.md`](docs/comparison.md) for where Claudette sits
relative to other agents.

**What Claudette isn't going to become:** a hosted SaaS, a plugin
marketplace, a VS Code extension, a multi-cloud-provider abstraction.
Proposals in those directions will be politely declined — the whole
point is to stay small and local.

## Development setup

```bash
git clone https://github.com/mrdushidush/claudette
cd claudette
cargo build --release
```

You'll need Ollama running locally for any end-to-end testing. See
the main [`README.md`](README.md#hardware-requirements) for model
requirements.

## Before you open a PR

Run these three checks. They're the same three the CI workflow runs,
so if they're green locally, CI will be green too:

```bash
cargo fmt --all --check
cargo clippy --all-targets --no-deps -- -D warnings
cargo test --lib
```

All three must pass. Tests currently sit at **514 passing, 4 ignored
on Windows** (hook tests that use POSIX `printf`) — a PR that drops
the pass count needs a justification in the description.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/). Every
commit on `main` uses one of these prefixes:

- `feat:` — new user-visible functionality
- `fix:` — bug fixes
- `refactor:` — internal reorganisation, no behavioural change
- `docs:` — README / CHANGELOG / `docs/*` edits
- `test:` — test-only changes
- `style:` — formatting only (`cargo fmt`)
- `chore:` — release prep, dep bumps, housekeeping
- `ci:` — changes under `.github/workflows/`

Keep the first line under 72 chars; prose in the body is encouraged
when the WHY is non-obvious. Look at `git log` for examples — the
existing history is the style guide.

## Adding a new tool

1. Add a JSON schema entry to the relevant `src/tools/<group>.rs` (or
   create a new group if none fits).
2. Add a handler `run_my_tool(input: &str) -> Result<String, String>`
   in the same module.
3. Wire it into the `dispatch` match at the top of the module.
4. For a new group: add a `ToolGroup` variant in
   `src/tool_groups.rs`, register the group's schemas and dispatcher
   in `src/tools.rs` (follow the 12 existing groups as templates).
5. Add at least one unit test covering the happy path and one
   covering a known failure mode (missing parameter, invalid JSON,
   boundary condition).

Document the group in the README's "70+ tools across 12 on-demand
groups" table so users can discover it.

## Adding a new tool group — permission tier

Every tool has a permission tier in `src/tool_groups.rs`:

- **ReadOnly** — auto-allowed. Pure reads, no side effects.
- **WorkspaceWrite** — auto-allowed. Writes stay under
  `~/.claudette/` (notes, todos, scratch code files, saved sessions).
- **DangerFullAccess** — user-prompted every call. Shell, arbitrary
  file edits, git commits/pushes, destructive network operations.

Default to the most restrictive tier that works. Prompting the user
is annoying; not prompting them for something that mutates the repo
is worse.

## Testing guidelines

- Tests that mutate environment variables must acquire
  `crate::test_env_lock()` to serialize with other env-mutating
  tests. Parallel cargo test without this lock produces flakes.
- Fixture-based tests (handler tests against canned JSON) should
  keep fixtures under `tests/fixtures/<group>/` — never inline
  5 KB of JSON into a `#[test]`.
- Opt-in live tests (tests that hit the real Ollama / Google / etc)
  go behind `#[ignore]` with a doc comment explaining what env vars
  or credentials are needed. Never in CI; run manually with
  `cargo test -- --ignored`.

## Reporting bugs

File at <https://github.com/mrdushidush/claudette/issues> with:

1. What you did — the exact command line or prompt that triggered it.
2. What you expected.
3. What actually happened — full error message if there is one.
4. Your setup — OS, Ollama version, model names.

A minimal reproducer is worth more than a paragraph of description.

## License

Claudette is licensed under Apache-2.0. By contributing, you agree
that your contributions are licensed under the same terms. No CLA,
no copyright assignment — just the implicit licence grant from the
license file.
