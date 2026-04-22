<!-- Thanks for opening a PR! CONTRIBUTING.md covers the full guide;
     this template is the quick version. -->

## What changed

<!-- One paragraph. Lead with the user-visible behavior change
     (or "no behavior change — refactor / docs / test"). -->

## Why

<!-- What problem does this solve? Link to an issue if one exists. -->

## Checks

<!-- All three must be green locally before pushing. CI enforces them. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets --no-deps -- -D warnings`
- [ ] `cargo test --lib`
- [ ] Updated `CHANGELOG.md` under `[Unreleased]` if this is a user-visible change

## Notes for the reviewer

<!-- Anything subtle worth a second pair of eyes: tricky edge case,
     a deviation from existing patterns, a known trade-off. -->
