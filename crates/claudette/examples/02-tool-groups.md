# 02 — Tool groups and `enable_tools`

Claudette ships 70+ tools but only advertises ~17 "core" tools by
default. The rest are loaded on demand via a synthetic meta-tool the
model can call: `enable_tools(group)`. This example walks through the
mechanic so you can see it happen live.

## Why on-demand?

Every tool schema the model sees costs input tokens. Advertising all
70 tools at once runs ~25 KB of JSON schema on every turn. Core-only
is ~4.7 KB. For a chatty REPL session that never touches GitHub or
markets data, that 80% reduction is real savings.

## Groups at a glance

| Group | Tools | Typical use |
|-------|-------|-------------|
| `core` (always on) | 17 | Notes, todos, files, time, web search, code gen |
| `git` | 8 | status, diff, log, add, commit, branch, checkout, push |
| `ide` | 3 | Open in editor / file manager / browser |
| `search` | 3 | Glob, grep, fetch-and-strip-HTML |
| `advanced` | 3 | Bash, `edit_file`, `spawn_agent` |
| `facts` | 4 | Wikipedia, Open-Meteo weather |
| `registry` | 4 | crates.io, npmjs |
| `github` | 6 | PRs/issues/code search |
| `markets` | 7 | TradingView quotes, Algorand ASA |
| `telegram` | 3 | Bot send/poll/photo |
| `calendar` | 5 | Google Calendar CRUD + RSVP |
| `schedule` | 4 | One-shot + recurring reminders |
| `gmail` | 4 | Read-only Gmail (list/search/read/labels) |

## Example — triggering a group

The model decides which groups to enable based on the user prompt. A
weather question, for instance, reliably triggers the `facts` group:

```
$ claudette "What's the weather in Tel Aviv right now?"
  ▸ enable_tools({"group": "facts"})
  ▸ get_weather_current({"latitude": 32.0853, "longitude": 34.7818})
It's 22°C and partly cloudy in Tel Aviv. Humidity is 65%, wind from
the north-west at 14 km/h. No precipitation.

⚡ iter=3 in=6210 out=88
```

Three iterations: (1) model calls `enable_tools("facts")`, (2)
receives the expanded schema and calls `get_weather_current`, (3)
summarizes the JSON response.

## Listing available tools

Inside the REPL or TUI, the `/tools` slash command lists every
advertisable tool — core plus every optional group with the
`enable_tools` invocation to turn it on:

```
> /tools
✨ secretary tools (core 17 + 12 optional groups)
  ⚡ core (always loaded)
    • get_current_time: Returns the current date, time, weekday, and timezone.
    • get_capabilities: Show the secretary's config, available tools, and limits.
    • note_create, note_list, note_read, note_delete
    • todo_add, todo_list, todo_complete, todo_uncomplete, todo_delete
    • read_file, write_file, list_dir
    • web_search, generate_code, spawn_agent

  ⚡ git — 8 tool(s), enable with enable_tools({group: "git"})
    • git_status, git_diff, git_log, git_add, git_commit, git_branch, git_checkout, git_push

  ⚡ facts — 4 tool(s), enable with enable_tools({group: "facts"})
    • wikipedia_search, wikipedia_summary, weather_current, weather_forecast

  [… 10 more groups: ide, search, advanced, registry, github, markets,
     telegram, calendar, schedule, gmail …]

  core schema: 4711 chars — enabling a group grows this temporarily
```

Output is the **advertisable** surface, not the live registry state
— `/tools` builds a fresh `ToolRegistry` for display because the live
registry is behind the conversation loop's borrow. To check what's
currently enabled in the session, call `get_capabilities` through
the model (or just ask "what tools do you have right now?").

Groups stay enabled until the session ends or `/clear` is called. A
second weather question in the same session is a direct
`get_weather_current` call — no re-enable needed.

## Pre-loading for Telegram and TUI

In `--telegram` and `--tui` modes the user can't confirm permissions
turn-by-turn, so Claudette auto-enables the "safe" groups at startup:
`markets`, `facts`, `search`, `advanced`, `git`. See `src/run.rs`
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
