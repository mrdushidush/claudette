# 02 — Tool groups and `enable_tools`

Claudette ships ~80 tools but only advertises 3 "core" tools by
default. The rest are loaded on demand via a synthetic meta-tool the
model can call: `enable_tools(group)`. This example walks through the
mechanic so you can see it happen live.

## Why on-demand?

Every tool schema the model sees costs input tokens. Advertising all
~80 tools at once runs ~34 KB of JSON schema on every turn. Core-only
is ~0.8 KB. For a chatty REPL session that never touches GitHub or
the web, that reduction is real savings.

## Groups at a glance

| Group | Tools | Typical use |
|-------|-------|-------------|
| `core` (always on) | 3 | enable_tools, current time, load workspace rules |
| `files` | 3 | read_file, write_file, list_dir |
| `git` | 9 | status, diff, log, add, commit, branch, checkout, push, clone |
| `ide` | 3 | Open in editor / file manager / browser |
| `search` | 5 | repo_map, glob, grep, web_fetch, web_search |
| `advanced` | 7 | Bash (+background/status/tail), `edit_file`, `apply_diff`, `ask_user` |
| `facts` | 2 | Wikipedia, Open-Meteo weather |
| `registry` | 2 | crates.io, npmjs |
| `github` | 15 | PRs/issues/code search + forge mission tools |
| `schedule` | 4 | One-shot + recurring reminders |
| `quality` | 3 | run_tests, diagnostics, apply_patch |
| `gmail` / `calendar` / `telegram` | (integrations) | Only in `--features integrations` builds |

## Example — triggering a group

The model decides which groups to enable based on the user prompt. A
weather question, for instance, reliably triggers the `facts` group:

```
$ claudette "What's the weather in Tel Aviv right now?"
  ▸ enable_tools({"group": "facts"})
  ▸ weather({"location": "Tel Aviv"})
It's 22°C and partly cloudy in Tel Aviv. Humidity is 65%, wind from
the north-west at 14 km/h. No precipitation.

⚡ iter=3 in=6210 out=88
```

Three iterations: (1) model calls `enable_tools("facts")`, (2)
receives the expanded schema and calls `weather`, (3)
summarizes the JSON response.

## Listing available tools

Inside the REPL or TUI, the `/tools` slash command lists every
advertisable tool — core plus every optional group with the
`enable_tools` invocation to turn it on:

```
> /tools
✨ agent tools (core 3 + 20 optional groups)
  ⚡ core (always loaded)
    ✓ enable_tools: Load an optional tool group (git, ide, search, advanced).
    ✓ get_current_time: Current date, time, weekday, timezone.
    ✓ load_workspace_rules: Load CLAUDETTE.md / .claudette/instructions.md from the project ancestor chain

  ⚡ files — 3 tool(s), enable with enable_tools({group: "files"})
    ✓ read_file, write_file, list_dir

  ⚡ git — 9 tool(s), enable with enable_tools({group: "git"})
    ✓ git_status, git_diff, git_log, git_add, git_commit, git_branch, git_checkout, git_push, git_clone

  ⚡ facts — 2 tool(s), enable with enable_tools({group: "facts"})
    ✓ wikipedia, weather

  [… more groups: notes, todos, meta, ide, search, advanced,
     registry, github, telegram, calendar, schedule, gmail,
     recall, quality, semantic, vision, clipboard …]

  core schema: 827 chars — enabling a group grows this temporarily
```

Output is the **advertisable** surface, not the live registry state
— `/tools` builds a fresh `ToolRegistry` for display because the live
registry is behind the conversation loop's borrow. To check what's
currently enabled in the session, call `get_capabilities` through
the model (or just ask "what tools do you have right now?").

Groups stay enabled until the session ends or `/clear` is called. A
second weather question in the same session is a direct
`weather` call — no re-enable needed.

## Pre-loading for Telegram and TUI

In `--telegram` and `--tui` modes the user can't confirm permissions
turn-by-turn, so Claudette auto-enables the "safe" groups at startup:
`facts`, `search`, `advanced`, `git`. See `src/run.rs`
(Telegram) and `src/tui_worker.rs` (TUI) for the exact lists — both
call the same `ToolRegistry::enable` path.

## Disabling a group

There is no explicit `disable_tools` in v0.2.0 — once a group is on,
it stays on for the session. If you want a clean slate, `/clear` or
start a new REPL.

## Adding your own group

If you fork the repo and want to add a new tool group (say, `spotify`
or `notion`), the three-file change is: add the enum variant in
`src/tool_groups.rs`, write the handler module in `src/tools/<name>.rs`,
register the schema list in `src/tools.rs`'s `dispatch_tool` match.
Every existing group (12 of them) is a template.
