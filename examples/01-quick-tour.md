# 01 — Quick tour

Five one-shot prompts you can run the moment Claudette is installed.
Each prompt is a full `claudette "<prompt>"` invocation: Claudette
prints the reply and exits. Useful for smoke-testing your install and
for shell pipelines.

All transcripts below came from real runs against `qwen3.5:4b` (the
default Auto-preset brain) on a single 3060 Ti.

## 1. What time is it?

```
$ claudette "What time is it?"
It's Sunday, April 19, 2026 at 11:50 PM in the +03:00 timezone.

⚡ iter=2 in=5041 out=48
```

The `⚡` footer is the per-turn usage summary: `iter` is the number of
model iterations (here the model issued a `get_current_time` tool call
and then summarized the result, so `iter=2`), `in` and `out` are
estimated input / output tokens.

## 2. Arithmetic — no tool call

```
$ claudette "What is 25 * 16?"
25 * 16 = 400

⚡ iter=1 in=2448 out=12
```

Simple arithmetic is handled without a tool call (`iter=1` confirms it
— the model answered directly). You can tell Claudette isn't
round-tripping through a calculator tool by the low output tokens.

## 3. List a directory

```
$ claudette "What files are in D:/dev/claudette/src?"
Here are the files in `D:/dev/claudette/src`:

**Rust source files:**
- agents.rs
- api.rs
- brain_selector.rs
- codet.rs
...
- voice.rs

**Directories:**
- runtime/
- tools/

⚡ iter=2 in=? out=?
```

This triggered a `list_dir` call. On Windows, you can use forward- or
back-slashes freely — path normalization happens in the tool layer.

## 4. Git status from a repo cwd

```
$ cd /path/to/my/repo
$ claudette "Show the current git status"
  ▸ git: using "D:\\Program Files\\Git\\mingw64\\bin\\git.exe", args=["status", "--short", "--branch"]
You're on the `main` branch and have 21 commits ahead of `origin/main`.
There are no modified, staged, or untracked files in the working directory.

⚡ iter=3 in=8791 out=79
```

The `▸ git:` line comes from the tool layer itself — Claudette shows
the exact command it invoked. `iter=3` here = the model enabled the
`git` group, called `git_status`, then summarized.

Note: `git` is not a core group, so the model has to
`enable_tools("git")` on the first turn. See
[`02-tool-groups.md`](02-tool-groups.md) for more on this mechanism.

## 5. Read-and-summarize a file

```
$ claudette "Read D:/dev/claudette/README.md and list some slash commands available in claudette"
Based on the README, here are the slash commands available in Claudette:

## REPL & TUI Commands

```
/help                Show this list.
/agents              List available sub-agent types.
/status              Session info + token counts.
...
/exit                Leave the REPL.
```
...

⚡ iter=2 in=11122 out=360
```

`read_file` fetches the README, the model synthesizes a summary. The
`in` number jumps (README is ~23 KB) but stays well under the 16 K
token `num_ctx` default.

## Try the REPL

```
$ claudette
You are talking to Claudette. Type /help for commands, /exit to quit.

>
```

The REPL autosaves to `~/.claudette/sessions/last.json` after every
turn. Resume with `claudette --resume` or `claudette -r`.
