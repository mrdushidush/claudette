# 05 — Code generation with Codet

Every call to `generate_code` routes through **Codet** — a separate LLM
pipeline with its own coder model, a syntax check, and a surgical
SEARCH/REPLACE fix loop. This example walks through what that looks
like end-to-end.

## Why a sidecar?

The main brain model is tuned for tool-calling and short, fast replies.
A dedicated coder model (default `qwen3-coder:30b`) is tuned for
correctness on multi-line code. Routing code-writing through a
specialist keeps brain context small and code quality high.

## 1. Basic Python generation

From the REPL:

```
> write a python script that downloads the top 10 Hacker News stories and prints their titles

  ▸ generate_code({"language": "python", "path": "hn_top10.py", "spec": "..."})
    Codet: writing hn_top10.py using qwen3-coder:30b
    Codet: syntax check — python -m py_compile hn_top10.py
    Codet: ok (1 attempt)

I generated `hn_top10.py` in your workspace. It pulls the top-stories
list from the Hacker News API, fetches each story's JSON in turn, and
prints the titles. No external deps; just `urllib`.
```

On the host's filesystem, the file lands in `~/.claudette/files/` by
default. The exact path Codet wrote to is printed in the reply above
the summary.

## 2. The fix loop in action

Sometimes the first draft has a syntax error. When Python's
`py_compile` catches one, Codet falls into a surgical SEARCH/REPLACE
loop — not a full regeneration:

```
  ▸ generate_code({"language": "rust", "path": "fibonacci.rs", ...})
    Codet: writing fibonacci.rs using qwen3-coder:30b
    Codet: syntax check — rustc --emit=metadata fibonacci.rs
    Codet: error — expected `;`, found `}`  at line 12
    Codet: surgical fix attempt 1/3
      search:  fn main() {
                 let n = 10
                 println!("{}", fib(n));
               }
      replace: fn main() {
                 let n = 10;
                 println!("{}", fib(n));
               }
    Codet: syntax check — rustc --emit=metadata fibonacci.rs
    Codet: ok (2 attempts)
```

Why surgical? An Aider-style SEARCH/REPLACE patch is ~50 output tokens
per attempt, vs ~5000 for full-file regeneration. The coder model
stays small and focused on the broken region.

On hard failures (3 attempts exhausted), Codet falls back to full-file
regeneration once. If that also fails, it reports honestly instead of
claiming success.

## 3. Languages supported for syntax check

| Language | Checker | Notes |
|----------|---------|-------|
| Python | `python -m py_compile` | Needs `python3` on PATH. |
| Rust | `rustc --emit=metadata` | Uses stable toolchain. |
| JavaScript | `node --check` | Falls back to plain parse if no `node`. |
| TypeScript | `tsc --noEmit` | Needs `tsc` on PATH. |
| HTML | regex validation | Lightweight; doesn't run a browser. |

Other extensions write through unchecked — Codet produces the file, no
syntax-check loop runs.

## 4. Validating an existing file

`/validate <path>` runs Codet on a file that's already on disk. Useful
after manual edits, or to diagnose a failing generation:

```
> /validate hn_top10.py
Codet: syntax check — python -m py_compile hn_top10.py
Codet: ok

The file parses cleanly.
```

## 5. VRAM-constrained hosts

On an 8 GB VRAM card the default coder (`qwen3-coder:30b`) needs the
brain model evicted before it loads. Codet does this automatically —
the swap cost is ~5-10s on a 3060 Ti. If that's too slow:

```bash
export CLAUDETTE_CODER_MODEL=qwen2.5-coder:14b   # smaller, fits alongside brain
export OLLAMA_MAX_LOADED_MODELS=1                # forces strict swap
```

Or pin the coder for a session:

```
> /coder qwen2.5-coder:14b
```

## 6. Running associated tests

If the generated file has a matching test file nearby (`test_foo.py`
for `foo.py`, `foo_test.rs` for `foo.rs`), Codet runs the test suite
after syntax-check succeeds:

```
Codet: syntax check — python -m py_compile hn_top10.py
Codet: test runner — pytest tests/
Codet: 3 passed in 0.21s
```

Fails in tests do NOT re-enter the surgical fix loop — they're reported
to the user as text. Tests are a quality signal, not a regeneration
trigger.

## 7. Disabling Codet

If the fix loop gets in your way (you just want the raw output, no
validation):

```bash
export CLAUDETTE_VALIDATE_CODE=false
```

`generate_code` then writes the file and returns. No syntax check, no
fix loop, no tests.

## 8. Known limitations

- **Brownfield editing** — Codet does not read an existing file, diff,
  and merge. `generate_code` is currently a whole-file operation. Use
  the `advanced/edit_file` tool for in-place search-and-replace.
- **Multi-file generation** — one file per call. The model can chain
  calls, but there's no atomic "generate a project skeleton" op.
- **Benchmark state** — brownfield correctness is ~67% honest quality
  on a 6-task audit (see v0.1.0 CHANGELOG known-limitations). The
  automatic grader only checks file-exists / size / syntax; humans
  found subtle spec deviations in 2 of 6.
