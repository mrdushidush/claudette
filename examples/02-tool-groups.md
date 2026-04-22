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

## Inspecting what's currently enabled

Inside the REPL or TUI, the `/tools` slash command lists every tool
grouped by capability:

```
> /tools

CORE (always enabled)
  add_numbers, get_current_time, get_capabilities, enable_tools, ...

git                  8 tools    DISABLED
ide                  3 tools    DISABLED
search               3 tools    DISABLED
advanced             3 tools    DISABLED
facts                4 tools    ENABLED
registry             4 tools    DISABLED
github               6 tools    DISABLED
markets              7 tools    DISABLED
telegram             3 tools    DISABLED
calendar             5 tools    DISABLED
schedule             4 tools    DISABLED
gmail                4 tools    DISABLED
```

Groups stay enabled until the session ends or `/clear` is called. A
second weather question in the same session is a direct
`get_weather_current` call — no re-enable needed.

## Pre-loading for Telegram

In Telegram mode the user can't confirm permissions turn-by-turn, so
Claudette auto-enables the "safe" groups at startup:
`markets`, `facts`, `search`, `ide`, `git`. See
`src/telegram_mode.rs` for the exact list — any changes there flow
through the codebase automatically.

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
