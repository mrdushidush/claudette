//! Tool definitions for the secretary, in Ollama's native tool-call schema.
//!
//! Each tool is declared as a JSON object compatible with Ollama's
//! `/api/chat` `tools` parameter. `dispatch_tool` is the sync entry point
//! `SecretaryToolExecutor` calls to actually run them.
//!
//! Storage layout (created on first write):
//!
//! ```text
//! ~/.claudette/
//! ├── notes/
//! │   └── 2026-04-08T11-30-15-call-mom-tomorrow.md
//! ├── files/
//! │   └── (sandboxed scratch dir for write_file)
//! └── todos.json
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Per-group sub-modules. Each exports `schemas()` and `dispatch()`; see the
// group-module contract at the top of `registry.rs`.
mod registry;

// ────────────────────────────────────────────────────────────────────────────
// Tool registry — advertised to the model on every request
// ────────────────────────────────────────────────────────────────────────────

#[must_use]
pub fn secretary_tools_json() -> Value {
    let mut tools: Vec<Value> = json!([
        // ── Core ────────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "get_current_time",
                "description": "Returns the current date, time, weekday, and timezone.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "note_create",
                "description": "Save a note with a title, body, and optional tags.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Note title" },
                        "body":  { "type": "string", "description": "Note content" },
                        "tags":  { "type": "string", "description": "Comma-separated tags (e.g. 'work,project,urgent')" }
                    },
                    "required": ["title", "body"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "note_list",
                "description": "List saved notes with titles, previews, and tags. Optionally filter by tag or search substring, and limit results.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tag":    { "type": "string", "description": "Filter by tag (case-insensitive)" },
                        "search": { "type": "string", "description": "Substring match against title or body (case-insensitive)" },
                        "limit":  { "type": "integer", "description": "Maximum notes to return (default 50)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "note_read",
                "description": "Read the full body of a saved note by its id (filename returned from note_list).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Note id from note_list (e.g. '2026-04-14T10-30-45-meeting.md')" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "note_delete",
                "description": "Delete a note by its id (filename from note_list). This is irreversible.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Note id from note_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_add",
                "description": "Add a task to the todo list.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Task description" }
                    },
                    "required": ["text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_list",
                "description": "List todos with their status and IDs. By default lists all; pass pending_only to hide completed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pending_only": { "type": "boolean", "description": "If true, hide completed todos (default false)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_complete",
                "description": "Mark a todo as done by its ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_uncomplete",
                "description": "Un-mark a completed todo (set done back to false) by its ID.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_delete",
                "description": "Delete a todo by its ID. This is irreversible.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Todo ID from todo_list" }
                    },
                    "required": ["id"]
                }
            }
        },
        // ── File ops ────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file under the user's home directory (max 100 KB).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path (absolute or ~/)" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write plain text / config / data to ~/.claudette/files/ (notes, JSON, YAML, TOML, MD, TXT, XML, INI). REFUSES code files (.py .rs .js .ts .html .css .go .java .c .cpp .rb .php .sh .sql etc) — for code you MUST use generate_code instead so the specialised coder + validator pipeline runs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":    { "type": "string", "description": "Filename or path under the sandbox" },
                        "content": { "type": "string", "description": "Text content to write" }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and folders in a directory under the user's home.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path (absolute or ~/)" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_capabilities",
                "description": "Show the secretary's config, available tools, and limits. Use for 'what can you do' questions.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        // ── Web ─────────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web via Brave Search. Returns results with title, URL, snippet, and extra context. Use for any current-information question.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "count": { "type": "number", "description": "Number of results (default 5, max 20)" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch a URL and return cleaned visible text (HTML stripped, max 8 KB).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch (http/https)" }
                    },
                    "required": ["url"]
                }
            }
        },
        // ── Search ──────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "glob_search",
                "description": "Find files by glob pattern under the user's home (e.g. **/*.py).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.py', 'Downloads/*.pdf')" }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep_search",
                "description": "Search file contents for a substring (case-insensitive) under a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Text to search for" },
                        "path":    { "type": "string", "description": "Directory to search (default: home)" }
                    },
                    "required": ["pattern"]
                }
            }
        },
        // ── IDE ─────────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "open_in_editor",
                "description": "Open a file in the default editor (invokes `code` on PATH), optionally at a line number.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" },
                        "line": { "type": "number", "description": "Line number (optional)" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "reveal_in_explorer",
                "description": "Show a file or folder in the system file manager.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File or folder path" }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "open_url",
                "description": "Open a URL or local file in the default browser. Accepts http/https URLs, file:// URIs, or absolute local file paths (e.g. C:\\Users\\...\\page.html).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL (http/https/file://) or absolute local file path" }
                    },
                    "required": ["url"]
                }
            }
        },
        // ── Git ─────────────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "git_status",
                "description": "Show working tree status (modified, staged, untracked files).",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_diff",
                "description": "Show file changes (unstaged by default, or staged).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":   { "type": "string",  "description": "Limit to this file (optional)" },
                        "staged": { "type": "boolean", "description": "Show staged changes instead" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_log",
                "description": "Show recent commit history. Use detail=true for full info (hash, author, date, message body).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "count":  { "type": "number",  "description": "Number of commits (default 10)" },
                        "path":   { "type": "string",  "description": "Limit to this file (optional)" },
                        "detail": { "type": "boolean", "description": "Show full commit info: hash, author, date, files changed (default false)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_add",
                "description": "Stage files for the next commit.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paths": { "type": "string", "description": "Space-separated file paths to stage" }
                    },
                    "required": ["paths"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_commit",
                "description": "Commit staged changes. If message is omitted, auto-generates one from the staged diff.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "Commit message (optional — auto-generated from diff if omitted)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_branch",
                "description": "List all branches, or create a new one if name is given.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "New branch name (omit to list)" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_checkout",
                "description": "Switch to a different branch.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Branch name or commit" }
                    },
                    "required": ["target"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_push",
                "description": "Push commits to the remote repository.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        // ── Shell + edit ─────────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command. Requires user confirmation. Use for system tasks the other tools can't handle.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to execute" }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace text in an existing file under the user's home. Requires confirmation. For creating new files use write_file or generate_code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":     { "type": "string", "description": "File path (absolute or ~/)" },
                        "old_text": { "type": "string", "description": "Exact text to find and replace" },
                        "new_text": { "type": "string", "description": "Replacement text" }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        // ── Code generation ─────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "generate_code",
                "description": "Generate code using the specialized coder model and write it to a file. USE THIS instead of write_file for any code. Supports Python, Rust, JavaScript, TypeScript, HTML, CSS. Auto-validates syntax and tests. The file is written to disk; reply with a SHORT confirmation (path + 1 sentence). DO NOT paste the generated code in your reply — it bloats the conversation and the user can already open the file. BROWNFIELD: when the user mentions an existing file the new code must match (e.g. 'add tests for X.py', 'extend X.py', 'refactor X.py'), ALWAYS list those file paths in `reference_files` so the coder can read the real API instead of inventing one.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "description":     { "type": "string", "description": "What code to write — include language, functions, tests needed" },
                        "filename":        { "type": "string", "description": "Filename (e.g. 'calc.py', 'lib.rs', 'app.ts'). Extension sets the language." },
                        "reference_files": { "type": "array", "items": { "type": "string" }, "description": "Existing file paths the coder MUST read before writing (real class/method names, signatures, exceptions). Pass each path as the user typed it — '~/.claudette/files/X.py', './X.py', or 'X.py'. Up to 4 files; oversize files are auto-truncated." }
                    },
                    "required": ["description", "filename"]
                }
            }
        },
        // ── Agent delegation ────────────────────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "spawn_agent",
                "description": "Delegate a task to a specialized agent. 'researcher' for web/file/code research, 'gitops' for git workflows, 'reviewer' for code review.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_type": { "type": "string", "enum": ["researcher", "gitops", "reviewer"], "description": "Agent type" },
                        "task":       { "type": "string", "description": "Task description for the agent" },
                        "auto":       { "type": "boolean", "description": "Skip confirmation prompts for dangerous tools (default false)" }
                    },
                    "required": ["agent_type", "task"]
                }
            }
        },
        // ── Sprint 9 Phase 0a — facts (Wikipedia + Open-Meteo) ──────────
        {
            "type": "function",
            "function": {
                "name": "wikipedia_search",
                "description": "Search Wikipedia for article titles matching a query. Returns top 5 hits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "wikipedia_summary",
                "description": "Get a plain-text summary of a Wikipedia article by exact title.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Exact article title (use wikipedia_search first if unsure)" }
                    },
                    "required": ["title"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "weather_current",
                "description": "Current weather for a city or 'lat,lon'. No API key needed. Uses Open-Meteo.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name (e.g. 'Paris') or 'lat,lon'" }
                    },
                    "required": ["location"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "weather_forecast",
                "description": "Multi-day weather forecast for a city or 'lat,lon'. No API key needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name or 'lat,lon'" },
                        "days":     { "type": "number", "description": "Number of days (1-7, default 3)" }
                    },
                    "required": ["location"]
                }
            }
        },
        // Registry group (crate_info, crate_search, npm_info, npm_search)
        // lives in src/tools/registry.rs and is appended to this array below.
        // ── Sprint 9 Phase 0a — GitHub (PAT via GITHUB_TOKEN env var) ───
        {
            "type": "function",
            "function": {
                "name": "gh_list_my_prs",
                "description": "List open pull requests I authored. Requires GITHUB_TOKEN in env.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "gh_list_assigned_issues",
                "description": "List open issues assigned to me. Requires GITHUB_TOKEN in env.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "gh_get_issue",
                "description": "Get a GitHub issue or PR by owner/repo/number.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":  { "type": "string", "description": "Repo owner (user or org)" },
                        "repo":   { "type": "string", "description": "Repo name" },
                        "number": { "type": "number", "description": "Issue or PR number" }
                    },
                    "required": ["owner", "repo", "number"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "gh_create_issue",
                "description": "Create a new GitHub issue in a repo.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string", "description": "Repo owner" },
                        "repo":  { "type": "string", "description": "Repo name" },
                        "title": { "type": "string", "description": "Issue title" },
                        "body":  { "type": "string", "description": "Issue body (Markdown, optional)" }
                    },
                    "required": ["owner", "repo", "title"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "gh_comment_issue",
                "description": "Post a comment on a GitHub issue or PR.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":  { "type": "string", "description": "Repo owner" },
                        "repo":   { "type": "string", "description": "Repo name" },
                        "number": { "type": "number", "description": "Issue or PR number" },
                        "body":   { "type": "string", "description": "Comment body (Markdown)" }
                    },
                    "required": ["owner", "repo", "number", "body"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "gh_search_code",
                "description": "Search code across GitHub. Returns top 5 file matches.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "GitHub code search query (see docs)" }
                    },
                    "required": ["query"]
                }
            }
        },
        // ── Sprint 9 Phase 0b — markets (TradingView + vestige.fi) ──────
        {
            "type": "function",
            "function": {
                "name": "tv_get_quote",
                "description": "Get current price and % change for a stock/crypto/forex symbol via TradingView. Accepts bare tickers (BTC, AAPL) or qualified (BINANCE:BTCUSDT, NASDAQ:NVDA). Default market 'america'; use 'crypto' for coins.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Ticker — bare (BTC, AAPL) or with exchange (BINANCE:BTCUSDT)" },
                        "market": { "type": "string", "description": "Market: 'america' (default), 'crypto', 'forex', 'futures'" }
                    },
                    "required": ["symbol"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "tv_technical_rating",
                "description": "Get TradingView's technical rating (strong_buy/buy/neutral/sell/strong_sell) for a symbol at a given interval. Accepts bare tickers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol":   { "type": "string", "description": "Ticker — bare (BTC, AAPL) or with exchange (BINANCE:BTCUSDT)" },
                        "interval": { "type": "string", "description": "Interval: '1m','5m','15m','1h','4h','1d','1W','1M' (default '1d')" },
                        "market":   { "type": "string", "description": "Market: 'america' (default), 'crypto', 'forex', 'futures'" }
                    },
                    "required": ["symbol"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "tv_search_symbol",
                "description": "Search TradingView for a symbol by name or ticker. Returns top 5 hits with exchange.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Symbol or company name" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "tv_economic_calendar",
                "description": "Get upcoming economic calendar events from TradingView (CPI, FOMC, NFP, etc).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "days_ahead": { "type": "number", "description": "Days forward from today (default 7, max 30)" },
                        "countries":  { "type": "string", "description": "Comma-separated country codes (default 'US')" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "vestige_asa_info",
                "description": "Get current USD/ALGO price and 24h change for an Algorand ASA by its asset id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "asa_id": { "type": "number", "description": "Algorand Standard Asset ID (e.g. 31566704 for USDC)" }
                    },
                    "required": ["asa_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "vestige_search_asa",
                "description": "Search vestige.fi for an Algorand ASA by ticker or name. Returns top 5 matches with ids.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Ticker or name (e.g. 'USDC', 'OPUL')" }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "vestige_top_movers",
                "description": "Get top Algorand ASA movers from vestige.fi ('gainers' or 'losers' by 24h change).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "direction": { "type": "string", "description": "'gainers' (default) or 'losers'" },
                        "limit":     { "type": "number", "description": "Number of results (default 5, max 20)" }
                    },
                    "required": []
                }
            }
        },
        // ── Sprint 10 — Telegram bot group ────────────────────────────
        {
            "type": "function",
            "function": {
                "name": "tg_send",
                "description": "Send a text message via Telegram bot. Supports Markdown formatting.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "string", "description": "Telegram chat ID (user or group). Use tg_get_updates to discover chat IDs." },
                        "text":    { "type": "string", "description": "Message text (supports Markdown)" }
                    },
                    "required": ["chat_id", "text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "tg_get_updates",
                "description": "Poll recent messages/commands sent to the Telegram bot. Use this to discover chat IDs and read incoming messages.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "number", "description": "Max updates to return (default 10, max 100)" },
                        "offset": { "type": "number", "description": "Update offset — pass last update_id+1 to acknowledge previous updates" }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "tg_send_photo",
                "description": "Send a photo via Telegram bot by URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "string", "description": "Telegram chat ID" },
                        "url":     { "type": "string", "description": "Public URL of the image to send" },
                        "caption": { "type": "string", "description": "Optional caption for the photo" }
                    },
                    "required": ["chat_id", "url"]
                }
            }
        }
    ])
    .as_array()
    .cloned()
    .unwrap_or_default();
    tools.extend(registry::schemas());
    Value::Array(tools)
}

// ────────────────────────────────────────────────────────────────────────────
// Dispatcher — entry point called by SecretaryToolExecutor
// ────────────────────────────────────────────────────────────────────────────

pub fn dispatch_tool(name: &str, input: &str) -> Result<String, String> {
    // Per-group dispatchers get first crack; each returns Some(_) if it owns
    // the tool, None otherwise. The `match` below handles everything that
    // hasn't migrated to a sub-module yet.
    if let Some(result) = registry::dispatch(name, input) {
        return result;
    }

    match name {
        "get_current_time" => Ok(run_get_current_time()),
        // add_numbers removed from registry (model can do arithmetic).
        // Dispatch kept so old sessions with tool_calls still work.
        "add_numbers" => run_add_numbers(input),
        "note_create" => run_note_create(input),
        "note_list" => run_note_list(input),
        "note_read" => run_note_read(input),
        "note_delete" => run_note_delete(input),
        "todo_add" => run_todo_add(input),
        "todo_list" => run_todo_list(input),
        "todo_complete" => run_todo_complete(input),
        "todo_uncomplete" => run_todo_uncomplete(input),
        "todo_delete" => run_todo_delete(input),
        "read_file" => run_read_file(input),
        "write_file" => run_write_file(input),
        "list_dir" => run_list_dir(input),
        "get_capabilities" => Ok(run_get_capabilities()),
        "web_search" => run_web_search(input),
        "glob_search" => run_glob_search(input),
        "grep_search" => run_grep_search(input),
        "web_fetch" => run_web_fetch(input),
        "open_in_editor" => run_open_in_editor(input),
        "reveal_in_explorer" => run_reveal_in_explorer(input),
        "open_url" => run_open_url(input),
        "git_status" => run_git_status(),
        "git_diff" => run_git_diff(input),
        "git_log" => run_git_log(input),
        "git_add" => run_git_add(input),
        "git_commit" => run_git_commit(input),
        "git_branch" => run_git_branch(input),
        "git_checkout" => run_git_checkout(input),
        "git_push" => run_git_push(),
        "bash" => run_bash(input),
        "edit_file" => run_edit_file(input),
        "generate_code" => run_generate_code(input),
        "spawn_agent" => run_spawn_agent(input),
        // ── Sprint 9 Phase 0a — facts group ────────────────────────────
        "wikipedia_search" => run_wikipedia_search(input),
        "wikipedia_summary" => run_wikipedia_summary(input),
        "weather_current" => run_weather_current(input),
        "weather_forecast" => run_weather_forecast(input),
        // Registry group (crate_info, crate_search, npm_info, npm_search)
        // is handled by the early-return above via registry::dispatch.
        // ── Sprint 9 Phase 0a — github group ───────────────────────────
        "gh_list_my_prs" => run_gh_list_my_prs(),
        "gh_list_assigned_issues" => run_gh_list_assigned_issues(),
        "gh_get_issue" => run_gh_get_issue(input),
        "gh_create_issue" => run_gh_create_issue(input),
        "gh_comment_issue" => run_gh_comment_issue(input),
        "gh_search_code" => run_gh_search_code(input),
        // ── Sprint 9 Phase 0b — markets group ──────────────────────────
        "tv_get_quote" => run_tv_get_quote(input),
        "tv_technical_rating" => run_tv_technical_rating(input),
        "tv_search_symbol" => run_tv_search_symbol(input),
        "tv_economic_calendar" => run_tv_economic_calendar(input),
        "vestige_asa_info" => run_vestige_asa_info(input),
        "vestige_search_asa" => run_vestige_search_asa(input),
        "vestige_top_movers" => run_vestige_top_movers(input),
        // ── Sprint 10 — telegram group ─────────────────────────────────
        "tg_send" => run_tg_send(input),
        "tg_get_updates" => run_tg_get_updates(input),
        "tg_send_photo" => run_tg_send_photo(input),
        other => Err(format!("unknown tool: {other}")),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Time & math (the original two)
// ────────────────────────────────────────────────────────────────────────────

fn run_get_current_time() -> String {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    // Monday = 1 .. Sunday = 7 (ISO 8601).
    let dow = now.weekday().number_from_monday();
    // "Tuesday, April 14, 2026 at 3:42 PM" — a natural string the model can
    // drop into a response without re-formatting.
    let human = {
        let hour12 = match now.hour() % 12 {
            0 => 12,
            h => h,
        };
        let ampm = if now.hour() < 12 { "AM" } else { "PM" };
        format!(
            "{}, {} {}, {} at {}:{:02} {}",
            now.format("%A"),
            now.format("%B"),
            now.day(),
            now.year(),
            hour12,
            now.minute(),
            ampm,
        )
    };
    json!({
        "iso8601": now.to_rfc3339(),
        "weekday": now.format("%A").to_string(),
        "weekday_num": dow,
        "date": now.format("%Y-%m-%d").to_string(),
        "time": now.format("%H:%M:%S").to_string(),
        "timezone": now.format("%:z").to_string(),
        "unix_timestamp": now.timestamp(),
        "human": human,
    })
    .to_string()
}

fn run_add_numbers(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("add_numbers: invalid JSON input ({e}): {input}"))?;
    let a = v
        .get("a")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("add_numbers: missing or non-numeric 'a' in {input}"))?;
    let b = v
        .get("b")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("add_numbers: missing or non-numeric 'b' in {input}"))?;
    Ok(json!({ "a": a, "b": b, "sum": a + b }).to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Storage helpers — ~/.claudette layout
// ────────────────────────────────────────────────────────────────────────────

fn user_home() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
}

fn claudette_home() -> PathBuf {
    user_home().join(".claudette")
}

fn notes_dir() -> PathBuf {
    claudette_home().join("notes")
}

fn todos_path() -> PathBuf {
    claudette_home().join("todos.json")
}

/// Scratch directory the secretary is allowed to write into.
/// Sits next to notes/ and todos.json so it's clearly within the
/// claudette data home and easy for the user to inspect or wipe.
fn files_dir() -> PathBuf {
    claudette_home().join("files")
}

fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir {}: {e}", path.display()))
}

/// Convert a title into a filesystem-safe slug. Lowercase, alphanumerics
/// and hyphens only, hyphens collapsed, max 40 chars.
fn slugify(text: &str) -> String {
    let raw: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed: String = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed: String = collapsed.chars().take(40).collect();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Notes (one .md file per note)
// ────────────────────────────────────────────────────────────────────────────

fn run_note_create(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_create: invalid JSON ({e}): {input}"))?;
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'title'")?
        .to_string();
    let body = v
        .get("body")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'body'")?
        .to_string();
    let tags_str = v.get("tags").and_then(Value::as_str).unwrap_or("");
    let tags: Vec<&str> = tags_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    ensure_dir(&notes_dir())?;
    let now = chrono::Local::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    let slug = slugify(&title);
    let filename = format!("{ts}-{slug}.md");
    let path = notes_dir().join(&filename);

    use std::fmt::Write;
    let mut content = format!("# {title}\n\nCreated: {}\n", now.to_rfc3339());
    if !tags.is_empty() {
        let _ = writeln!(content, "Tags: {}", tags.join(", "));
    }
    let _ = writeln!(content, "\n{body}");
    fs::write(&path, content).map_err(|e| format!("note_create: write failed: {e}"))?;

    let mut result = json!({
        "ok": true,
        "id": filename,
        "path": path.display().to_string(),
        "title": title,
    });
    if !tags.is_empty() {
        result["tags"] = json!(tags);
    }
    Ok(result.to_string())
}

fn run_note_list(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let filter_tag = v
        .get("tag")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let search = v
        .get("search")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let limit = v
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(50, |n| n as usize);

    let dir = notes_dir();
    if !dir.exists() {
        return Ok(json!({ "count": 0, "notes": [] }).to_string());
    }

    // Collect (filename, title, tags, preview, body_for_search).
    let mut entries: Vec<(String, String, Vec<String>, String, String)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("read notes dir: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let content = fs::read_to_string(&path).unwrap_or_default();
        let title = content.lines().find(|l| l.starts_with("# ")).map_or_else(
            || filename.clone(),
            |l| l.trim_start_matches("# ").to_string(),
        );
        let tags: Vec<String> = content
            .lines()
            .find(|l| l.starts_with("Tags:"))
            .map(|l| {
                l.trim_start_matches("Tags:")
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Apply tag filter.
        if let Some(ref ft) = filter_tag {
            if !tags.iter().any(|t| t.to_lowercase() == *ft) {
                continue;
            }
        }
        // Apply search filter against title + full content (case-insensitive).
        if let Some(ref q) = search {
            let hay = format!("{}\n{}", title, content).to_lowercase();
            if !hay.contains(q) {
                continue;
            }
        }

        let preview: String = content
            .lines()
            .find(|l| {
                !(l.starts_with('#')
                    || l.starts_with("Created:")
                    || l.starts_with("Tags:")
                    || l.trim().is_empty())
            })
            .map(|s| s.chars().take(80).collect::<String>())
            .unwrap_or_default();
        entries.push((filename, title, tags, preview, content));
    }
    // Newest first by filename (ISO timestamp prefix sorts naturally)
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let total = entries.len();
    entries.truncate(limit);

    let json_entries: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, (id, title, tags, preview, _))| {
            let mut entry = json!({
                "index": i + 1,
                "id": id,
                "title": title,
                "preview": preview,
            });
            if !tags.is_empty() {
                entry["tags"] = json!(tags);
            }
            entry
        })
        .collect();

    let mut result = json!({
        "count": json_entries.len(),
        "total": total,
        "notes": json_entries,
    });
    if let Some(ref ft) = filter_tag {
        result["filtered_by_tag"] = json!(ft);
    }
    if let Some(ref q) = search {
        result["search"] = json!(q);
    }
    if total > json_entries.len() {
        result["truncated"] = json!(true);
    }
    Ok(result.to_string())
}

fn run_note_read(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_read: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("note_read: missing 'id' (filename from note_list)")?;
    // Reject path separators — id must be a bare filename.
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("note_read: invalid id '{id}' (must be a filename)"));
    }
    let path = notes_dir().join(id);
    if !path.exists() {
        return Err(format!("note_read: no note with id '{id}'"));
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("note_read: read failed: {e}"))?;

    let title = content.lines().find(|l| l.starts_with("# ")).map_or_else(
        || id.to_string(),
        |l| l.trim_start_matches("# ").to_string(),
    );
    let created = content
        .lines()
        .find(|l| l.starts_with("Created:"))
        .map(|l| l.trim_start_matches("Created:").trim().to_string())
        .unwrap_or_default();
    let tags: Vec<String> = content
        .lines()
        .find(|l| l.starts_with("Tags:"))
        .map(|l| {
            l.trim_start_matches("Tags:")
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // Body = everything after the metadata block. Skip lines that start with
    // `#`, `Created:`, `Tags:`, or are blank. The first non-metadata line and
    // everything after it is the body.
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in content.lines() {
        if !in_body {
            let is_meta = line.starts_with('#')
                || line.starts_with("Created:")
                || line.starts_with("Tags:")
                || line.trim().is_empty();
            if is_meta {
                continue;
            }
            in_body = true;
        }
        body_lines.push(line);
    }
    let body = body_lines.join("\n").trim_end().to_string();

    let mut result = json!({
        "ok": true,
        "id": id,
        "title": title,
        "body": body,
    });
    if !created.is_empty() {
        result["created"] = json!(created);
    }
    if !tags.is_empty() {
        result["tags"] = json!(tags);
    }
    Ok(result.to_string())
}

fn run_note_delete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_delete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("note_delete: missing 'id' (filename from note_list)")?;
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!(
            "note_delete: invalid id '{id}' (must be a filename)"
        ));
    }
    let path = notes_dir().join(id);
    if !path.exists() {
        return Err(format!("note_delete: no note with id '{id}'"));
    }
    fs::remove_file(&path).map_err(|e| format!("note_delete: remove failed: {e}"))?;
    Ok(json!({ "ok": true, "id": id, "deleted": true }).to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Todos (single todos.json file)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct Todo {
    id: String,
    text: String,
    done: bool,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
}

fn load_todos() -> Result<Vec<Todo>, String> {
    let path = todos_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = fs::read_to_string(&path).map_err(|e| format!("read todos: {e}"))?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&s).map_err(|e| format!("parse todos.json: {e}"))
}

fn save_todos(todos: &[Todo]) -> Result<(), String> {
    ensure_dir(&claudette_home())?;
    let s = serde_json::to_string_pretty(todos).map_err(|e| format!("serialize todos: {e}"))?;
    fs::write(todos_path(), s).map_err(|e| format!("write todos: {e}"))
}

fn run_todo_add(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_add: invalid JSON ({e}): {input}"))?;
    // Prefer "text"; accept "content" as a fallback for older prompts.
    let text = v
        .get("text")
        .or_else(|| v.get("content"))
        .and_then(Value::as_str)
        .ok_or("todo_add: missing 'text'")?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("todo_add: 'text' cannot be empty".to_string());
    }

    let mut todos = load_todos()?;
    let now = chrono::Local::now();
    let id = format!("t_{}", now.timestamp_millis());
    todos.push(Todo {
        id: id.clone(),
        text: text.clone(),
        done: false,
        created_at: now.to_rfc3339(),
        completed_at: None,
    });
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text }).to_string())
}

fn run_todo_list(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let pending_only = v
        .get("pending_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let todos = load_todos()?;
    let total = todos.len();
    let pending = todos.iter().filter(|t| !t.done).count();
    let view: Vec<Value> = todos
        .iter()
        .enumerate()
        .filter(|(_, t)| !pending_only || !t.done)
        .map(|(i, t)| {
            let mut obj = json!({
                "index": i + 1,
                "id": t.id,
                "text": t.text,
                "done": t.done,
                "created_at": t.created_at,
            });
            if let Some(ref c) = t.completed_at {
                obj["completed_at"] = json!(c);
            }
            obj
        })
        .collect();
    let mut result = json!({
        "count": view.len(),
        "total": total,
        "pending": pending,
        "todos": view,
    });
    if pending_only {
        result["pending_only"] = json!(true);
    }
    Ok(result.to_string())
}

fn run_todo_complete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_complete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_complete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let mut updated = None;
    for t in &mut todos {
        if t.id == id {
            t.done = true;
            t.completed_at = Some(chrono::Local::now().to_rfc3339());
            updated = Some(t.text.clone());
            break;
        }
    }
    let text = updated.ok_or_else(|| format!("todo_complete: no todo with id '{id}'"))?;
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text, "done": true }).to_string())
}

fn run_todo_uncomplete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_uncomplete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_uncomplete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let mut updated = None;
    for t in &mut todos {
        if t.id == id {
            t.done = false;
            t.completed_at = None;
            updated = Some(t.text.clone());
            break;
        }
    }
    let text = updated.ok_or_else(|| format!("todo_uncomplete: no todo with id '{id}'"))?;
    save_todos(&todos)?;

    Ok(json!({ "ok": true, "id": id, "text": text, "done": false }).to_string())
}

fn run_todo_delete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("todo_delete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("todo_delete: missing 'id'")?
        .to_string();

    let mut todos = load_todos()?;
    let before = todos.len();
    let removed_text = todos.iter().find(|t| t.id == id).map(|t| t.text.clone());
    todos.retain(|t| t.id != id);
    if todos.len() == before {
        return Err(format!("todo_delete: no todo with id '{id}'"));
    }
    save_todos(&todos)?;

    Ok(json!({
        "ok": true,
        "id": id,
        "text": removed_text.unwrap_or_default(),
        "deleted": true,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Self-introspection (get_capabilities)
//
// Without this tool, the model has no way to answer "what can you do" or
// "how much memory do you have" except by guessing — and we measured it
// guessing wrong (claiming "no fixed context window" when in fact num_ctx
// is 4096 with a sliding-window truncator). Returning real values from a
// tool fixes self-description without bloating the system prompt (which
// the README explicitly warns suppresses tool calling on qwen3.5:9b).
// ────────────────────────────────────────────────────────────────────────────

fn run_get_capabilities() -> String {
    // Sprint 8: report tools as core + optional groups. We build a fresh
    // ToolRegistry for inspection; the live runtime's registry may have
    // additional groups enabled, but this snapshot is still the right
    // answer for "what could you do if you needed to", which is what the
    // model is really asking.
    let registry = crate::tool_groups::ToolRegistry::new();
    let core_names = registry.core_tool_names();
    let groups_summary: Vec<Value> = crate::tool_groups::ToolGroup::all()
        .iter()
        .map(|g| {
            json!({
                "name": g.name(),
                "summary": g.summary(),
                "tools": registry.group_tool_names(*g),
            })
        })
        .collect();
    let total_tools = core_names.len()
        + crate::tool_groups::ToolGroup::all()
            .iter()
            .map(|g| registry.group_tool_names(*g).len())
            .sum::<usize>();

    json!({
        "name": "Claudette",
        "kind": "personal AI secretary",
        "model": crate::run::current_model(),
        "runtime": "crate::ConversationRuntime over Ollama /api/chat",
        "context_window": {
            "num_ctx_tokens": crate::api::current_num_ctx(),
            "num_predict_tokens": crate::api::current_num_predict(),
            "auto_compaction_threshold_tokens": crate::run::compact_threshold(),
            "notes": "Auto-compaction summarises old turns when cumulative input tokens cross the threshold; the most recent turns stay verbatim. A char-based sliding-window truncator inside api.rs is the in-iteration safety net.",
        },
        "tools": {
            "total": total_tools,
            "core": core_names,
            "optional_groups": groups_summary,
            "note": "Optional group tools are only advertised after you call enable_tools(group) — they cut the per-turn schema cost when unused.",
        },
        "sandbox": {
            "read": "user $HOME (/home/<user> or C:\\Users\\<user>) — symlinks/junctions resolved as such, system dirs not blocked but ACL-protected anyway",
            "write": files_dir().display().to_string(),
            "rationale": "writes are sandboxed to ~/.claudette/files/ so the secretary cannot overwrite the user's real documents by accident or hallucination",
        },
        "storage": {
            "notes": notes_dir().display().to_string(),
            "todos": todos_path().display().to_string(),
            "scratch_files": files_dir().display().to_string(),
            "session": crate::run::default_session_path().display().to_string(),
        },
        "version": env!("CARGO_PKG_VERSION"),
    })
    .to_string()
}

// ────────────────────────────────────────────────────────────────────────────
// Web search (Brave Search API)
// ────────────────────────────────────────────────────────────────────────────

fn run_web_search(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("web_search: invalid JSON ({e}): {input}"))?;
    let query = v
        .get("query")
        .and_then(Value::as_str)
        .ok_or("web_search: missing 'query'")?
        .to_string();
    let count = v
        .get("count")
        .and_then(Value::as_i64)
        .unwrap_or(5)
        .clamp(1, 20) as usize;

    // Legacy: the original env var was BRAVE_API_KEY (not BRAVE_TOKEN).
    // Check both the unified secret store AND the legacy name.
    let api_key = crate::secrets::read_secret("brave")
        .or_else(|_| {
            std::env::var("BRAVE_API_KEY")
                .map(|v| v.trim().to_string())
                .map_err(|_| String::new())
        })
        .map_err(|_| {
            format!(
                "web_search: Brave API key not found. Get one at https://brave.com/search/api/ \
                 and either export BRAVE_API_KEY or save it to {}",
                crate::secrets::secret_file_path("brave").display()
            )
        })?;

    let count_str = count.to_string();
    let client = external_http_client()?;
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query.as_str()), ("count", count_str.as_str())])
        .header("Accept", "application/json")
        .header("X-Subscription-Token", &api_key)
        .send()
        .map_err(|e| format!("web_search: request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "web_search: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("web_search: parse failed: {e}"))?;

    // Main web results — richer extraction.
    let results: Vec<Value> = data
        .pointer("/web/results")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(count)
                .map(|r| {
                    let mut result = json!({
                        "title": r.get("title").and_then(Value::as_str).unwrap_or(""),
                        "url": r.get("url").and_then(Value::as_str).unwrap_or(""),
                        "description": r.get("description").and_then(Value::as_str).unwrap_or(""),
                    });
                    // Extra snippets — Brave provides additional text fragments
                    // that often contain the direct answer.
                    if let Some(extras) = r.get("extra_snippets").and_then(Value::as_array) {
                        let snippets: Vec<&str> =
                            extras.iter().filter_map(Value::as_str).take(2).collect();
                        if !snippets.is_empty() {
                            result["extra_snippets"] = json!(snippets);
                        }
                    }
                    // Age of the result (e.g. "2 days ago").
                    if let Some(age) = r.get("age").and_then(Value::as_str) {
                        result["age"] = json!(age);
                    }
                    result
                })
                .collect()
        })
        .unwrap_or_default();

    let mut response = json!({
        "query": query,
        "count": results.len(),
        "results": results,
    });

    // Infobox — Brave sometimes provides a Wikipedia-style summary card.
    if let Some(infobox) = data.pointer("/infobox") {
        if let Some(title) = infobox.pointer("/results/0/title").and_then(Value::as_str) {
            let desc = infobox
                .pointer("/results/0/long_desc")
                .or_else(|| infobox.pointer("/results/0/description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            response["infobox"] = json!({
                "title": title,
                "description": desc,
            });
        }
    }

    Ok(response.to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// File ops (read_file, write_file, list_dir)
//
// Sandboxing policy (intentional, narrow by default — loosen on demand):
//   • read_file / list_dir: allowed anywhere under the user's $HOME.
//     Lets the secretary research and summarize the user's own documents
//     without exposing system files (/etc, C:\Windows, etc).
//   • write_file: allowed ONLY under ~/.claudette/files/. The secretary
//     gets its own scratch space; it can't overwrite anything important
//     by accident or hallucination. Users who want a draft moved to e.g.
//     Documents can copy it themselves.
//
// Path normalization is manual (no canonicalize) so it works for paths
// that don't yet exist (write_file targets) and avoids Windows UNC noise
// (\\?\C:\...). This does NOT defend against symlink escape — acceptable
// threat model for a local secretary running on the user's own machine.
// ────────────────────────────────────────────────────────────────────────────

const MAX_FILE_BYTES: usize = 100 * 1024; // 100 KB
const MAX_LIST_ENTRIES: usize = 200;

/// Expand a leading `~` to the user's home directory. Other tildes are left
/// alone (matching shell behaviour). `pub(crate)` so the `/validate` slash
/// command can reuse the same tilde logic as the file-ops tools.
pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        user_home().join(rest)
    } else if input == "~" {
        user_home()
    } else {
        PathBuf::from(input)
    }
}

/// Resolve `.` and `..` components without touching the filesystem.
/// Absolute paths stay absolute; relative paths stay relative (joined to
/// CWD by the caller if needed). Leading `..` on a relative path is kept.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // Pop only if the last component is a real directory name.
                // Don't pop a Prefix, RootDir, or another ParentDir.
                let popped =
                    matches!(out.components().next_back(), Some(Component::Normal(_))) && out.pop();
                if !popped {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Normalize an input path string, expanding `~` and resolving `.`/`..`.
/// Relative paths are made absolute by joining to the current working dir.
fn resolve_input_path(input: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(input);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        let cwd = std::env::current_dir().map_err(|e| format!("get cwd: {e}"))?;
        cwd.join(expanded)
    };
    Ok(normalize_path(&absolute))
}

// ────────────────────────────────────────────────────────────────────────────
// Reference-file extraction (brownfield API-matching — Sprint 13)
//
// When the user asks `generate_code` to write tests/code that references an
// existing file, extract every path-like token from the description, resolve
// each under $HOME or the scratch dir, and return the file contents. The
// collector is intentionally conservative: it will only surface files that
// (a) syntactically look like a path, and (b) actually exist on disk.
// ────────────────────────────────────────────────────────────────────────────

/// File extensions we'll include as reference context.
const REF_EXTENSIONS: &[&str] = &[
    "py", "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "css", "json", "toml",
    "yaml", "yml", "md", "txt", "sh", "bash", "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp",
    "rb", "php", "sql", "xml", "ini", "cfg", "conf",
];

/// Max files, per-file byte cap, and total byte cap. Keeps the coder prompt
/// below ~70 KB even when the user references several modules.
const REF_MAX_FILES: usize = 4;
const REF_MAX_BYTES_PER_FILE: usize = 16 * 1024;
const REF_MAX_BYTES_TOTAL: usize = 64 * 1024;

/// File extensions that `write_file` refuses, redirecting the brain to
/// `generate_code` instead (Sprint 13.3 — bulletproof code routing).
///
/// Strict subset of `REF_EXTENSIONS`: only languages where the brain has no
/// business writing the file directly. Config/data formats (json, toml, yaml,
/// md, txt, xml, ini, cfg, conf) stay on `write_file` because the brain CAN
/// write those coherently and they don't go through Codet validation.
///
/// Why refuse: the 4b brain produces tiny stubs for code, bypassing the 30b
/// coder + Codet validation entirely. Sprint 13.3 v3 task #55 collapsed to a
/// 747-byte 2-function stub of a 12-function module via this exact path.
const CODE_EXTENSIONS: &[&str] = &[
    "py", "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "css", "go", "java", "c",
    "cpp", "cc", "cxx", "h", "hpp", "rb", "php", "sh", "bash", "sql",
];

fn is_code_extension(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let lower = e.to_ascii_lowercase();
            CODE_EXTENSIONS.contains(&lower.as_str())
        })
}

// ────────────────────────────────────────────────────────────────────────────
// Per-turn user-prompt path stash (Sprint 13.2 — bypass-the-brain brownfield)
//
// The brain summarises the user prompt before constructing tool calls and
// regularly drops file paths. Even with the explicit `reference_files` schema
// param, the 4b brain rarely populates it. Solution: extract paths from the
// raw user prompt at the entry point (REPL / single-shot / Telegram / TUI),
// stash them here, and merge in `collect_reference_files`. Bypasses the brain
// entirely. Each entry point overwrites the stash before submitting the turn.
// ────────────────────────────────────────────────────────────────────────────

use std::sync::{Mutex, OnceLock};

static CURRENT_TURN_PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn current_turn_paths_mu() -> &'static Mutex<Vec<String>> {
    CURRENT_TURN_PATHS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Replace the per-turn path list. Called from each entry point with the paths
/// extracted from the raw user prompt. An empty Vec clears the stash, which is
/// the right thing to do for non-brownfield prompts (no leakage between turns).
pub fn set_current_turn_paths(paths: Vec<String>) {
    if let Ok(mut g) = current_turn_paths_mu().lock() {
        *g = paths;
    }
}

/// Read the current stash. Returns an empty Vec if poisoned (defensive — we'd
/// rather degrade to "no refs" than panic the agent loop).
pub(crate) fn current_turn_paths() -> Vec<String> {
    current_turn_paths_mu()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Scan the raw user prompt for path tokens and keep only those that resolve
/// to an existing file under the read policy. Used by entry points to populate
/// the per-turn stash.
#[must_use]
pub fn extract_user_prompt_paths(prompt: &str) -> Vec<String> {
    extract_path_candidates(prompt)
        .into_iter()
        .filter(|t| resolve_reference(t).is_some())
        .collect()
}

/// Collect reference files for the coder prompt. Three sources, in priority order:
///   1. **Per-turn stash** — paths the system pre-extracted from the raw user
///      prompt (Sprint 13.2). Most reliable: bypasses the brain entirely.
///   2. `explicit` — paths the brain passed via the `reference_files` tool param.
///      Useful when the brain follows the schema instruction.
///   3. `description` — fallback path-scan for when the brain forgets BOTH the
///      param AND the path didn't make it into the user message verbatim.
///
/// All three go through the same `validate_read_path` policy and size caps,
/// and dedup by absolute path so a path hit on multiple sources only loads once.
pub(crate) fn collect_reference_files(
    explicit: &[&str],
    description: &str,
) -> Vec<crate::codet::ReferenceFile> {
    let mut out: Vec<crate::codet::ReferenceFile> = Vec::new();
    let mut seen_abs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;

    let stash_iter = current_turn_paths().into_iter();
    let explicit_iter = explicit.iter().map(|s| (*s).to_string());
    let scanner_iter = extract_path_candidates(description).into_iter();
    for token in stash_iter.chain(explicit_iter).chain(scanner_iter) {
        if out.len() >= REF_MAX_FILES {
            break;
        }
        let Some(resolved) = resolve_reference(&token) else {
            continue;
        };
        if !seen_abs.insert(resolved.clone()) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&resolved) else {
            continue;
        };
        let trimmed = truncate_content(content);
        if total_bytes.saturating_add(trimmed.len()) > REF_MAX_BYTES_TOTAL {
            break;
        }
        total_bytes += trimmed.len();
        out.push(crate::codet::ReferenceFile {
            path: token,
            content: trimmed,
        });
    }
    out
}

fn truncate_content(mut content: String) -> String {
    if content.len() > REF_MAX_BYTES_PER_FILE {
        // Truncate at a char boundary, then annotate.
        let mut cut = REF_MAX_BYTES_PER_FILE;
        while cut > 0 && !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        content.push_str("\n... [truncated — file continues]\n");
    }
    content
}

/// Break a free-form description into path-shaped candidate tokens, stripping
/// surrounding quotes/brackets/trailing punctuation. Does NOT check the
/// filesystem — `resolve_reference` does that.
fn extract_path_candidates(text: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let mut buf = String::new();
    for c in text.chars() {
        if c.is_whitespace()
            || matches!(
                c,
                ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | '<' | '>'
            )
        {
            if !buf.is_empty() {
                raw.push(std::mem::take(&mut buf));
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        raw.push(buf);
    }

    raw.into_iter()
        .filter_map(|t| {
            // Strip trailing sentence punctuation (em-dash, en-dash, etc).
            let trimmed = t
                .trim_end_matches(|c: char| {
                    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '—' | '–' | ')')
                })
                .to_string();
            if trimmed.is_empty() {
                return None;
            }
            // URLs look like paths-with-extensions but aren't reachable via
            // the filesystem — drop them before they trip resolve_reference.
            if trimmed.contains("://") {
                return None;
            }
            if looks_like_path(&trimmed) || has_code_extension(&trimmed) {
                Some(trimmed)
            } else {
                None
            }
        })
        .collect()
}

/// `true` iff the token uses explicit path syntax (tilde, absolute, dotted
/// relative, or a Windows drive letter). URLs are excluded.
fn looks_like_path(s: &str) -> bool {
    if s.contains("://") {
        return false;
    }
    if s.starts_with("~/") || s.starts_with("~\\") {
        return true;
    }
    if s.starts_with("./") || s.starts_with(".\\") || s.starts_with("../") || s.starts_with("..\\")
    {
        return true;
    }
    if s.starts_with('/') || s.starts_with('\\') {
        return true;
    }
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn has_code_extension(s: &str) -> bool {
    Path::new(s)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let lower = e.to_ascii_lowercase();
            REF_EXTENSIONS.contains(&lower.as_str())
        })
}

/// Resolve a token to an absolute path on disk, or `None` if no readable
/// file exists under $HOME, the scratch dir, or the current working dir.
fn resolve_reference(token: &str) -> Option<PathBuf> {
    // Explicit path: use the same read-policy as read_file.
    if looks_like_path(token) {
        return validate_read_path(token).ok().filter(|p| p.is_file());
    }
    // Bare filename with a code extension: try scratch dir then cwd.
    if !has_code_extension(token) {
        return None;
    }
    for dir in [
        files_dir(),
        std::env::current_dir().unwrap_or_else(|_| files_dir()),
    ] {
        let candidate = dir.join(token);
        if candidate.is_file() {
            let as_string = candidate.to_string_lossy().to_string();
            if let Ok(validated) = validate_read_path(&as_string) {
                return Some(validated);
            }
        }
    }
    None
}

/// Validate a read/list path: must resolve under the user's home directory
/// OR the current working directory (so the researcher agent can access
/// project files outside $HOME).
fn validate_read_path(input: &str) -> Result<PathBuf, String> {
    let resolved = resolve_input_path(input)?;
    let home = normalize_path(&user_home());
    if resolved.starts_with(&home) {
        return Ok(resolved);
    }
    // Also allow paths under the current working directory (project root).
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_norm = normalize_path(&cwd);
        if resolved.starts_with(&cwd_norm) {
            return Ok(resolved);
        }
    }
    Err(format!(
        "path is outside both $HOME ({}) and the working directory; reads are restricted for safety",
        home.display()
    ))
}

/// Validate a write path: must resolve under `~/.claudette/files/`.
fn validate_write_path(input: &str) -> Result<PathBuf, String> {
    let resolved = resolve_input_path(input)?;
    let scratch = normalize_path(&files_dir());
    if !resolved.starts_with(&scratch) {
        return Err(format!(
            "writes are sandboxed to {}. Use a path under that directory.",
            scratch.display()
        ));
    }
    Ok(resolved)
}

fn run_read_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("read_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("read_file: missing 'path'")?;

    let path = validate_read_path(path_str)?;

    let metadata = fs::metadata(&path)
        .map_err(|e| format!("read_file: stat {} failed: {e}", path.display()))?;
    if metadata.is_dir() {
        return Err(format!(
            "read_file: {} is a directory; use list_dir instead",
            path.display()
        ));
    }
    let size = metadata.len();
    if size > MAX_FILE_BYTES as u64 {
        return Err(format!(
            "read_file: {} is {size} bytes, exceeds {MAX_FILE_BYTES}-byte limit",
            path.display()
        ));
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("read_file: read {} failed: {e}", path.display()))?;

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": size,
        "content": content,
    })
    .to_string())
}

fn run_write_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("write_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("write_file: missing 'path'")?;
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .ok_or("write_file: missing 'content'")?;

    // Refuse code files. The brain (small, generalist) routinely writes
    // tiny code stubs that bypass the 30b coder + Codet validation. Force
    // the call through `generate_code` so the quality pipeline kicks in.
    // Brain reads the structured error and reroutes on the next turn.
    if is_code_extension(path_str) {
        return Err(format!(
            "write_file refuses code files (extension on '{path_str}'). \
             Use `generate_code` instead — it routes through the specialised \
             coder model and validates syntax+tests. Pass any existing files \
             the new code should match in `reference_files` so the coder \
             reads the real API."
        ));
    }

    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "write_file: content is {} bytes, exceeds {MAX_FILE_BYTES}-byte limit",
            content.len()
        ));
    }

    // Bare relative paths get resolved under the sandbox dir, NOT against
    // CWD. Reasoning: the model says "save it to dolphins-post.txt" and
    // expects it to land somewhere reasonable; resolving against
    // claudette's CWD (typically the workspace root) puts it outside
    // the sandbox and the call fails. By rooting bare relative paths under
    // ~/.claudette/files/ we make the most-common case Just Work.
    // Absolute and ~/-prefixed paths still flow through validate_write_path
    // unchanged so the user can still explicitly target a sub-folder.
    let resolved_input = if Path::new(path_str).is_absolute()
        || path_str.starts_with("~/")
        || path_str.starts_with("~\\")
    {
        path_str.to_string()
    } else {
        files_dir().join(path_str).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(&path, content)
        .map_err(|e| format!("write_file: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": content.len(),
    });

    // Codet post-write hook: if the file looks like code, validate it.
    // The validation is synchronous — it may hot-swap models in Ollama
    // (Claudette ↔ coder) and take 10-30 seconds for the full
    // parse→test→fix loop. The result is folded into the tool output so
    // Claudette sees it without any extra context cost.
    if let Some(validation) = crate::codet::validate_code_file(&path, &[]) {
        result["validation"] = validation.to_json();

        // Surface a warning directly to the user (stderr) if Codet
        // couldn't fix something — the user should know even if Claudette
        // decides not to mention it.
        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

fn run_list_dir(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("list_dir: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("list_dir: missing 'path'")?;

    let path = validate_read_path(path_str)?;

    let metadata = fs::metadata(&path)
        .map_err(|e| format!("list_dir: stat {} failed: {e}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("list_dir: {} is not a directory", path.display()));
    }

    let mut entries: Vec<(String, &'static str, u64)> = Vec::new();
    let read = fs::read_dir(&path)
        .map_err(|e| format!("list_dir: read {} failed: {e}", path.display()))?;
    for entry in read {
        let entry = entry.map_err(|e| format!("list_dir: entry error: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Use file_type() (does NOT follow links) for classification, not
        // metadata() (which follows). Windows legacy junction points like
        // "My Documents" or "Application Data" are reparse points whose
        // targets are ACL-locked; metadata() fails on them and used to
        // bucket them as `("unknown", 0)` or — worse — as `("file", 0)`
        // depending on the error path. file_type() reports them as
        // symlinks correctly.
        let (kind, size) = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => ("symlink", 0),
            Ok(ft) if ft.is_dir() => ("dir", 0),
            Ok(ft) if ft.is_file() => {
                // Only stat real files for size — metadata() can be
                // expensive (or fail with permission errors) on Windows.
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                ("file", size)
            }
            Ok(_) => ("other", 0),
            Err(_) => ("unknown", 0),
        };
        entries.push((name, kind, size));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let total = entries.len();
    let truncated = total > MAX_LIST_ENTRIES;
    if truncated {
        entries.truncate(MAX_LIST_ENTRIES);
    }

    let json_entries: Vec<Value> = entries
        .iter()
        .map(|(name, kind, size)| {
            json!({
                "name": name,
                "type": kind,
                "size": size,
            })
        })
        .collect();

    Ok(json!({
        "path": path.display().to_string(),
        "count": total,
        "truncated": truncated,
        "entries": json_entries,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// IDE tools — open_in_editor, reveal_in_explorer, open_url
//
// Fire-and-forget subprocesses. No output parsing — we just spawn the
// process and return immediately. The child process inherits no stdout/
// stderr handle so it doesn't block the REPL.
// ────────────────────────────────────────────────────────────────────────────

fn run_open_in_editor(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("open_in_editor: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("open_in_editor: missing 'path'")?;
    let line = v.get("line").and_then(Value::as_u64);

    let resolved = validate_read_path(path_str)?;
    let target = match line {
        Some(n) => format!("{}:{n}", resolved.display()),
        None => resolved.display().to_string(),
    };

    // On Windows, the default editor binary installs as `code.cmd` which
    // `Command::new("code")` can't find (CreateProcessW doesn't honour
    // PATHEXT). Use `cmd /C` wrapper on Windows to let the shell resolve it.
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "code", "--goto", &target])
            .spawn()
            .map_err(|e| format!("open_in_editor: failed to launch editor: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("code")
            .arg("--goto")
            .arg(&target)
            .spawn()
            .map_err(|e| format!("open_in_editor: failed to launch editor: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "opened": target,
    })
    .to_string())
}

fn run_reveal_in_explorer(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("reveal_in_explorer: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("reveal_in_explorer: missing 'path'")?;

    let resolved = validate_read_path(path_str)?;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", resolved.display()))
            .spawn()
            .map_err(|e| format!("reveal_in_explorer: failed to launch explorer: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // macOS: open -R; Linux: xdg-open on parent dir
        let parent = resolved.parent().unwrap_or(&resolved);
        std::process::Command::new("xdg-open")
            .arg(parent.as_os_str())
            .spawn()
            .map_err(|e| format!("reveal_in_explorer: failed to open file manager: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "revealed": resolved.display().to_string(),
    })
    .to_string())
}

fn run_open_url(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("open_url: invalid JSON ({e}): {input}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .ok_or("open_url: missing 'url'")?;

    // Accept http(s), file:// URIs, and bare local paths (e.g. HTML files
    // generated by Codet). Windows `start` and Linux `xdg-open` handle all
    // three — the old http-only guard prevented opening local files.
    let is_url =
        url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://");
    if !is_url && !Path::new(url).exists() {
        return Err(format!("open_url: not a URL or existing local file: {url}"));
    }
    let target = url;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .map_err(|e| format!("open_url: failed to open: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map_err(|e| format!("open_url: failed to open: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "opened": target,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Git tools
//
// All run `git` as a subprocess via `run_command_with_timeout` from
// `test_runner.rs`. CWD is the workspace root (where claudette was
// launched). Safety: destructive flags (--force, reset --hard, clean -f,
// branch -D) are rejected before they reach the subprocess.
// ────────────────────────────────────────────────────────────────────────────

use crate::test_runner::run_command_with_timeout;

/// Resolve the full path to `git.exe`. On Windows, git is often installed
/// under `Program Files` but NOT added to the system PATH (it's only in
/// Git Bash's internal PATH). `Command::new("git")` fails in that case.
///
/// Strategy: try `where git` first (works if git IS in PATH), then probe
/// known install locations. Caches the result via `OnceLock` so the
/// filesystem scan runs at most once per process.
fn resolve_git_path() -> String {
    use std::sync::OnceLock;
    static GIT_PATH: OnceLock<String> = OnceLock::new();
    GIT_PATH
        .get_or_init(|| {
            // 1. Try `where git` (works when git is in PATH).
            #[cfg(target_os = "windows")]
            {
                if let Ok(out) = std::process::Command::new("where").arg("git").output() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if let Some(path) = stdout.lines().next().map(str::trim) {
                        if !path.is_empty() && std::path::Path::new(path).exists() {
                            return path.to_string();
                        }
                    }
                }

                // 2. Probe known Git for Windows install locations.
                let drives = ["C:", "D:", "E:"];
                let suffixes = [
                    r"\Program Files\Git\cmd\git.exe",
                    r"\Program Files\Git\bin\git.exe",
                    r"\Program Files\Git\mingw64\bin\git.exe",
                    r"\Program Files (x86)\Git\cmd\git.exe",
                ];
                for drive in &drives {
                    for suffix in &suffixes {
                        let candidate = format!("{drive}{suffix}");
                        if std::path::Path::new(&candidate).exists() {
                            return candidate;
                        }
                    }
                }
            }
            "git".to_string()
        })
        .clone()
}

/// Run a git command from the workspace root (CWD). Returns the
/// `CommandResult` stdout on success, or an error with stderr.
///
/// On Windows, resolves git via `where git` first (handles spaces in
/// PATH like `D:\Program Files\Git\...`). Falls back to bare `git`.
fn run_git(args: &[&str]) -> Result<String, String> {
    let git_exe = resolve_git_path();
    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!("git: using {git_exe:?}, args={args:?}")),
    );
    let result = run_command_with_timeout(&git_exe, args, 30, None);
    if !result.success {
        eprintln!(
            "  {} {}",
            crate::theme::dim("▸"),
            crate::theme::dim(&format!(
                "git: failed — exit={:?} stderr={:?}",
                result.exit_code,
                result.stderr.chars().take(200).collect::<String>()
            )),
        );
    }
    if result.timed_out {
        return Err(format!(
            "git {}: timed out after 30s",
            args.first().unwrap_or(&"")
        ));
    }
    if !result.success {
        let err = if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            result.stderr.clone()
        };
        return Err(format!(
            "git {}: exit code {:?}\n{}",
            args.first().unwrap_or(&""),
            result.exit_code,
            err.chars().take(500).collect::<String>()
        ));
    }
    Ok(result.stdout)
}

/// Reject arguments that contain destructive git flags. Called before
/// every git tool dispatch. Better to over-block than to let a small
/// model accidentally force-push or hard-reset.
fn reject_destructive(args: &[&str]) -> Result<(), String> {
    let banned = [
        "--force",
        "-f",
        "--force-with-lease",
        "--hard",
        "--mixed", // reset --hard/--mixed
        "-D",      // branch -D (force delete)
        "--no-verify",
    ];
    for arg in args {
        for b in &banned {
            if arg == b {
                return Err(format!(
                    "git: destructive flag `{arg}` is blocked for safety. \
                     If you really need it, run git manually outside the secretary."
                ));
            }
        }
    }
    Ok(())
}

fn run_git_status() -> Result<String, String> {
    let output = run_git(&["status", "--short", "--branch"])?;
    Ok(json!({ "output": output }).to_string())
}

fn run_git_diff(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let staged = v.get("staged").and_then(Value::as_bool).unwrap_or(false);
    let path = v.get("path").and_then(Value::as_str);

    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    // Cap diff output so it doesn't blow the context window.
    args.push("--stat");
    args.push("--patch");
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let output = run_git(&args)?;
    // Truncate very large diffs.
    let truncated = output.len() > 8000;
    let visible: String = output.chars().take(8000).collect();
    Ok(json!({ "output": visible, "truncated": truncated }).to_string())
}

fn run_git_log(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let count = v.get("count").and_then(Value::as_u64).unwrap_or(10);
    let path = v.get("path").and_then(Value::as_str);
    let detail = v.get("detail").and_then(Value::as_bool).unwrap_or(false);

    let count_str = format!("-{count}");
    let format_str;
    let mut args = vec!["log", &count_str];

    if detail {
        // Rich format: hash, author, date, subject, body + file stats.
        format_str = "--format=%H %an (%ar)%n  %s%n%b".to_string();
        args.push(&format_str);
        args.push("--stat");
    } else {
        args.push("--oneline");
    }

    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let output = run_git(&args)?;
    // Truncate in detail mode since --stat can be verbose.
    if detail && output.len() > 6000 {
        let truncated: String = output.chars().take(6000).collect();
        Ok(json!({ "output": truncated, "truncated": true }).to_string())
    } else {
        Ok(json!({ "output": output }).to_string())
    }
}

fn run_git_add(input: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| format!("git_add: invalid JSON ({e}): {input}"))?;
    let paths_str = v
        .get("paths")
        .and_then(Value::as_str)
        .ok_or("git_add: missing 'paths'")?;

    let paths: Vec<&str> = paths_str.split_whitespace().collect();
    if paths.is_empty() {
        return Err("git_add: no paths specified".to_string());
    }
    // Block `git add -A` / `git add .` — too dangerous for this workspace.
    for p in &paths {
        if *p == "-A" || *p == "--all" || *p == "." {
            return Err(format!(
                "git_add: `{p}` is blocked — stage files explicitly by name to avoid \
                 accidentally adding .venv noise or secrets"
            ));
        }
    }

    let mut args = vec!["add"];
    args.extend(paths.iter());
    let output = run_git(&args)?;
    Ok(json!({ "ok": true, "staged": paths_str, "output": output }).to_string())
}

fn run_git_commit(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("git_commit: invalid JSON ({e}): {input}"))?;
    let message_param = v.get("message").and_then(Value::as_str).unwrap_or("");

    let message = if message_param.trim().is_empty() {
        // Auto-generate from staged diff.
        auto_commit_message()?
    } else {
        message_param.to_string()
    };

    let output = run_git(&["commit", "-m", &message])?;
    Ok(json!({ "ok": true, "message": message, "output": output }).to_string())
}

/// Generate a commit message from the currently staged diff.
fn auto_commit_message() -> Result<String, String> {
    let stat = run_git(&["diff", "--cached", "--stat"])?;
    if stat.trim().is_empty() {
        return Err("git_commit: nothing staged — run git_add first".to_string());
    }

    // Parse file names from stat output: lines like " src/tools.rs | 42 ++--"
    let files: Vec<&str> = stat
        .lines()
        .filter(|l| l.contains('|'))
        .map(|l| l.split('|').next().unwrap_or("").trim())
        .filter(|f| !f.is_empty())
        .collect();

    // Parse the summary line: "3 files changed, 45 insertions(+), 10 deletions(-)"
    let summary_line = stat.lines().last().unwrap_or("");
    let insertions = extract_stat_number(summary_line, "insertion");
    let deletions = extract_stat_number(summary_line, "deletion");

    // Build a concise message.
    let file_count = files.len();
    let file_list = if file_count <= 3 {
        files.join(", ")
    } else {
        format!("{}, {} and {} more", files[0], files[1], file_count - 2)
    };

    let stat_suffix = match (insertions, deletions) {
        (0, 0) => String::new(),
        (i, 0) => format!(" (+{i})"),
        (0, d) => format!(" (-{d})"),
        (i, d) => format!(" (+{i}, -{d})"),
    };

    Ok(format!("Update {file_list}{stat_suffix}"))
}

/// Extract a number from a git stat summary line by keyword prefix.
fn extract_stat_number(line: &str, keyword: &str) -> usize {
    // Pattern: "45 insertions(+)" — find the number before the keyword.
    for part in line.split(',') {
        let trimmed = part.trim();
        if trimmed.contains(keyword) {
            if let Some(num_str) = trimmed.split_whitespace().next() {
                if let Ok(n) = num_str.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

fn run_git_branch(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let name = v.get("name").and_then(Value::as_str);

    match name {
        Some(n) if !n.is_empty() => {
            reject_destructive(&[n])?;
            let output = run_git(&["branch", n])?;
            Ok(json!({ "ok": true, "created": n, "output": output }).to_string())
        }
        _ => {
            let output = run_git(&["branch", "-a"])?;
            Ok(json!({ "output": output }).to_string())
        }
    }
}

fn run_git_checkout(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("git_checkout: invalid JSON ({e}): {input}"))?;
    let target = v
        .get("target")
        .and_then(Value::as_str)
        .ok_or("git_checkout: missing 'target'")?;

    reject_destructive(&[target])?;
    let output = run_git(&["checkout", target])?;
    Ok(json!({ "ok": true, "checked_out": target, "output": output }).to_string())
}

fn run_git_push() -> Result<String, String> {
    // Stub confirmation — print a warning for now. Real PermissionMode::Prompt
    // will be wired in Sprint 2c when the permission system is tightened.
    eprintln!(
        "{} {}",
        crate::theme::warn(crate::theme::WARN_GLYPH),
        crate::theme::warn("git_push: pushing to remote...")
    );
    let output = run_git(&["push"])?;
    Ok(json!({ "ok": true, "output": output }).to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Shell + edit — requires DangerFullAccess (user confirmation via CLI prompt)
// ────────────────────────────────────────────────────────────────────────────

const BASH_OUTPUT_MAX_CHARS: usize = 8192;

fn run_bash(input: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| format!("bash: invalid JSON ({e}): {input}"))?;
    let command = v
        .get("command")
        .and_then(Value::as_str)
        .ok_or("bash: missing 'command'")?;

    if command.trim().is_empty() {
        return Err("bash: command is empty".to_string());
    }

    // Execute via the platform shell so pipes, redirects, and builtins work.
    #[cfg(target_os = "windows")]
    let (program, args) = ("cmd", vec!["/C", command]);
    #[cfg(not(target_os = "windows"))]
    let (program, args) = ("sh", vec!["-c", command]);

    let result = run_command_with_timeout(program, &args, 30, None);

    let stdout: String = result.stdout.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let stderr: String = result.stderr.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let truncated =
        result.stdout.len() > BASH_OUTPUT_MAX_CHARS || result.stderr.len() > BASH_OUTPUT_MAX_CHARS;

    Ok(json!({
        "exit_code": result.exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "timed_out": result.timed_out,
        "truncated": truncated,
    })
    .to_string())
}

fn run_edit_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("edit_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'path'")?;
    let old_text = v
        .get("old_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'old_text'")?;
    let new_text = v
        .get("new_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'new_text'")?;

    // $HOME-gated (broader than write_file's sandbox) because the user
    // explicitly confirmed via the permission prompt.
    let path = validate_read_path(path_str)?;

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("edit_file: read {} failed: {e}", path.display()))?;

    if !content.contains(old_text) {
        return Err(format!(
            "edit_file: old_text not found in {}. The text to replace must match exactly.",
            path.display()
        ));
    }

    let new_content = content.replacen(old_text, new_text, 1);
    fs::write(&path, &new_content)
        .map_err(|e| format!("edit_file: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": new_content.len(),
    });

    // Codet post-edit hook for code files (same as write_file).
    if let Some(validation) = crate::codet::validate_code_file(&path, &[]) {
        result["validation"] = validation.to_json();
        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Code generation — delegates to the coder model via Codet
// ────────────────────────────────────────────────────────────────────────────

fn run_generate_code(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("generate_code: invalid JSON ({e}): {input}"))?;
    let description = v
        .get("description")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'description'")?;
    let filename = v
        .get("filename")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'filename'")?;

    // Infer language from extension.
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("text");
    let language = match ext {
        "py" => "Python",
        "rs" => "Rust",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "php" => "PHP",
        "rb" => "Ruby",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "hpp" => "C++",
        "sh" | "bash" => "Bash",
        other => other,
    };

    // Collect reference files for the coder. Two signals:
    //   - `reference_files`: explicit array the brain passed (deterministic).
    //   - `description`: free-form scan for path tokens the brain mentioned in prose.
    // The explicit param is the contract; the scanner stays as a fallback so
    // brains that forget the param still get partial coverage.
    // Brownfield fix v2 (Sprint 13.1, 2026-04-18) — see project_sprint13_brownfield.
    let explicit_refs: Vec<&str> = v
        .get("reference_files")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let references = collect_reference_files(&explicit_refs, description);

    // Generate code via the coder model.
    let code = crate::codet::generate_code(description, language, &references)
        .ok_or("generate_code: coder model returned no usable output")?;

    // Write via the same sandbox logic as write_file (bare relative paths
    // resolve under ~/.claudette/files/).
    let resolved_input = if Path::new(filename).is_absolute()
        || filename.starts_with("~/")
        || filename.starts_with("~\\")
    {
        filename.to_string()
    } else {
        files_dir().join(filename).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(&path, &code)
        .map_err(|e| format!("generate_code: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": code.len(),
        "language": language,
        "generated_by": crate::codet::coder_model(),
        // Strong hint for the model: the file is on disk, do not paste
        "reply_hint": "File written. Reply with: file path + 1-sentence \
                       summary. DO NOT include the code in your response.",
    });

    // Run Codet validation (same as write_file post-write hook). Pass the
    // references so the fix-loop also sees the real API when repairing tests.
    if let Some(validation) = crate::codet::validate_code_file(&path, &references) {
        result["validation"] = validation.to_json();

        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Agent delegation
// ────────────────────────────────────────────────────────────────────────────

fn run_spawn_agent(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("spawn_agent: invalid JSON ({e}): {input}"))?;
    let type_str = v
        .get("agent_type")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'agent_type'")?;
    let agent_type = crate::agents::AgentType::parse(type_str).ok_or_else(|| {
        format!("spawn_agent: unknown agent type '{type_str}'. Use 'researcher' or 'gitops'.")
    })?;
    let task = v
        .get("task")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'task'")?;
    let auto_mode = v.get("auto").and_then(Value::as_bool).unwrap_or(false);

    crate::agents::spawn_agent(agent_type, task, auto_mode)
}

// ────────────────────────────────────────────────────────────────────────────
// Search tools — glob_search, grep_search
//
// Both walk the filesystem under the user's $HOME (same gate as read_file /
// list_dir). Caps are intentionally tight so a curious model can't sink a
// REPL turn into a 10-minute scan of every file the user has ever owned.
// ────────────────────────────────────────────────────────────────────────────

const MAX_GLOB_RESULTS: usize = 200;
const MAX_GREP_MATCHES: usize = 50;
const MAX_GREP_FILES: usize = 200;
const MAX_GREP_LINE_CHARS: usize = 200;

fn run_glob_search(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("glob_search: invalid JSON ({e}): {input}"))?;
    let raw_pattern = v
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or("glob_search: missing 'pattern'")?;

    // Resolve pattern to an absolute filesystem path. Three cases:
    //   - Absolute path → use as-is, then validate it stays under $HOME.
    //   - Tilde-prefixed → expand_tilde.
    //   - Bare relative pattern → join under $HOME.
    let resolved_pattern = if raw_pattern.starts_with("~/") || raw_pattern.starts_with("~\\") {
        expand_tilde(raw_pattern).display().to_string()
    } else if Path::new(raw_pattern).is_absolute() {
        raw_pattern.to_string()
    } else {
        user_home().join(raw_pattern).display().to_string()
    };

    // Sandbox check on the literal prefix (everything before the first glob
    // metachar). The literal prefix is the part of the path glob will
    // actually walk into; if THAT escapes $HOME we reject. Without this
    // check the user could pass `../etc/**/*` and walk outside $HOME.
    let prefix_end = resolved_pattern
        .find(['*', '?', '['])
        .unwrap_or(resolved_pattern.len());
    let literal_prefix = &resolved_pattern[..prefix_end];
    let literal_path = normalize_path(Path::new(literal_prefix));
    let home = normalize_path(&user_home());
    if !literal_path.starts_with(&home) {
        return Err(format!(
            "glob_search: pattern resolves outside $HOME ({}); searches are restricted for safety",
            home.display()
        ));
    }

    // Glob errors (bad pattern syntax) → user-facing error.
    let walker =
        glob::glob(&resolved_pattern).map_err(|e| format!("glob_search: bad pattern: {e}"))?;

    let mut paths: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in walker {
        if paths.len() >= MAX_GLOB_RESULTS {
            truncated = true;
            break;
        }
        // Permission errors and unreachable paths come back as Err — skip
        // them silently rather than failing the whole search.
        if let Ok(path) = entry {
            paths.push(path.display().to_string());
        }
    }
    paths.sort();

    Ok(json!({
        "pattern": resolved_pattern,
        "count": paths.len(),
        "truncated": truncated,
        "paths": paths,
    })
    .to_string())
}

fn run_grep_search(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("grep_search: invalid JSON ({e}): {input}"))?;
    let pattern = v
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or("grep_search: missing 'pattern'")?;
    if pattern.is_empty() {
        return Err("grep_search: pattern is empty".to_string());
    }
    let path_str = v.get("path").and_then(Value::as_str).unwrap_or("~");

    let root = validate_read_path(path_str)?;
    let metadata = fs::metadata(&root)
        .map_err(|e| format!("grep_search: stat {} failed: {e}", root.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "grep_search: {} is not a directory",
            root.display()
        ));
    }

    let needle = pattern.to_lowercase();
    let mut matches: Vec<Value> = Vec::new();
    let mut files_scanned: usize = 0;
    let mut truncated = false;

    // Iterative DFS over the directory tree. Skips hidden directories
    // (`.cache`, `.git`, etc.) so a personal-secretary grep doesn't drown
    // in dotfile noise. The MAX_GREP_FILES + MAX_GREP_MATCHES caps are the
    // belt-and-braces against runaway walks.
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Skip hidden entries (Unix dot-prefix; we don't try to detect
            // Windows hidden attribute, that needs a separate API call).
            if name_str.starts_with('.') {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                // Don't follow symlinks — could loop or escape sandbox.
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            // Bail-out conditions checked per-file so we always finish the
            // current entry's matches before stopping.
            if files_scanned >= MAX_GREP_FILES {
                truncated = true;
                break 'walk;
            }
            files_scanned += 1;

            // Skip oversized files — same 100 KB cap as read_file.
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES as u64 {
                continue;
            }
            // Read as text; binary files fail UTF-8 and get skipped.
            let Ok(content) = fs::read_to_string(&p) else {
                continue;
            };
            for (lineno, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    let snippet: String = line.chars().take(MAX_GREP_LINE_CHARS).collect();
                    matches.push(json!({
                        "file": p.display().to_string(),
                        "line": lineno + 1,
                        "text": snippet,
                    }));
                    if matches.len() >= MAX_GREP_MATCHES {
                        truncated = true;
                        break 'walk;
                    }
                }
            }
        }
    }

    Ok(json!({
        "pattern": pattern,
        "root": root.display().to_string(),
        "files_scanned": files_scanned,
        "match_count": matches.len(),
        "truncated": truncated,
        "matches": matches,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// web_fetch — pull a single URL, strip HTML, return cleaned text
//
// MVP: no scheme allowlist beyond http/https, no SSRF guard (the threat
// model is a local secretary on the user's own machine), no JS rendering.
// 8 KB cap on output keeps the context window safe even on giant pages.
// ────────────────────────────────────────────────────────────────────────────

const WEB_FETCH_MAX_CHARS: usize = 8192;
const WEB_FETCH_TIMEOUT_SECS: u64 = 15;

fn run_web_fetch(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("web_fetch: invalid JSON ({e}): {input}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .ok_or("web_fetch: missing 'url'")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "web_fetch: only http:// and https:// URLs are allowed, got: {url}"
        ));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("web_fetch: build http client: {e}"))?;

    let resp = client
        .get(url)
        .header("User-Agent", "claudette/1.0 (Claudette personal secretary)")
        .header("Accept", "text/html,application/xhtml+xml,text/plain")
        .send()
        .map_err(|e| format!("web_fetch: request failed: {e}"))?;

    let status = resp.status();
    let final_url = resp.url().to_string();
    if !status.is_success() {
        return Err(format!("web_fetch: HTTP {status} for {final_url}"));
    }
    let body = resp
        .text()
        .map_err(|e| format!("web_fetch: read body: {e}"))?;

    let cleaned = strip_html(&body);
    let total_chars = cleaned.chars().count();
    let truncated = total_chars > WEB_FETCH_MAX_CHARS;
    let visible: String = cleaned.chars().take(WEB_FETCH_MAX_CHARS).collect();

    Ok(json!({
        "url": final_url,
        "status": status.as_u16(),
        "chars": visible.chars().count(),
        "total_chars": total_chars,
        "truncated": truncated,
        "text": visible,
    })
    .to_string())
}

/// Strip HTML to plain text. Two-step pipeline:
///   1. Drop the contents of `<script>` and `<style>` blocks (they're
///      garbage for the model and dwarf the visible text on most modern
///      pages — Twitter is multiple MB of JSON-in-JS).
///   2. Remove all remaining tags via a `<` / `>` state machine, decode a
///      handful of common HTML entities, and collapse whitespace runs.
///
/// This is intentionally cheap and dependency-free. It will mangle some
/// pages (anything that abuses `<` literally outside an attribute, or pages
/// that use exotic entities). The 8 KB output cap limits the blast radius.
fn strip_html(html: &str) -> String {
    let no_scripts = strip_tag_block(html, "script");
    let no_styles = strip_tag_block(&no_scripts, "style");

    let mut out = String::with_capacity(no_styles.len());
    let mut in_tag = false;
    for c in no_styles.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let mut collapsed = String::with_capacity(decoded.len());
    let mut last_ws = true;
    for c in decoded.chars() {
        if c.is_whitespace() {
            if !last_ws {
                collapsed.push(' ');
                last_ws = true;
            }
        } else {
            collapsed.push(c);
            last_ws = false;
        }
    }
    collapsed.trim().to_string()
}

/// Remove every `<tag ...>...</tag>` block (case-insensitive on the tag
/// name). Used by `strip_html` for `<script>` and `<style>`. Substring
/// based — no regex dep — so it's a little dumb but the test coverage
/// pins down the cases that matter.
fn strip_tag_block(html: &str, tag: &str) -> String {
    let open_lower = format!("<{tag}");
    let close_lower = format!("</{tag}>");
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut cursor: usize = 0;

    while cursor < html.len() {
        let Some(rel_open) = lower[cursor..].find(&open_lower) else {
            out.push_str(&html[cursor..]);
            break;
        };
        let abs_open = cursor + rel_open;
        out.push_str(&html[cursor..abs_open]);
        // Find the matching close after the open. If absent, drop the rest.
        match lower[abs_open..].find(&close_lower) {
            Some(rel_close) => {
                cursor = abs_open + rel_close + close_lower.len();
            }
            None => break,
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Sprint 9 Phase 0a — external services (keyless + PAT)
//
// All HTTP calls go through `external_http_client()` which sets a sensible
// User-Agent (required by crates.io, polite for Wikipedia/npm) and a 15-sec
// timeout. Each `run_*` function:
//   • parses the input JSON,
//   • builds the request,
//   • maps HTTP errors to a descriptive `Err(String)`,
//   • returns a compact JSON string that Claudette can summarise.
//
// No cross-tool state — every call is self-contained. Token for GitHub
// tools is read from `GITHUB_TOKEN` (compatible with the GitHub CLI) and
// falls back to `CLAUDETTE_GITHUB_TOKEN`.
// ────────────────────────────────────────────────────────────────────────────

/// User-Agent sent on every external HTTP call. crates.io explicitly rejects
/// clients without a descriptive User-Agent (contact info required).
fn external_user_agent() -> String {
    format!(
        "claudette/{} (claudette; https://github.com/davidtzoar/claudette)",
        env!("CARGO_PKG_VERSION")
    )
}

pub(super) fn external_http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(external_user_agent())
        .build()
        .map_err(|e| format!("external http: build client failed: {e}"))
}

/// Resolve the GitHub token via the unified secret store. Checks
/// `CLAUDETTE_GITHUB_TOKEN`, then `GITHUB_TOKEN`, then
/// `~/.claudette/secrets/github.token`.
fn github_token() -> Result<String, String> {
    crate::secrets::read_secret("github").map_err(|_| {
        format!(
            "github: token not found. Create a fine-grained PAT at \
             https://github.com/settings/tokens and either export GITHUB_TOKEN \
             or save it to {}",
            crate::secrets::secret_file_path("github").display()
        )
    })
}

/// Generic "extract `key` as str from a JSON object, or a named error".
pub(super) fn extract_str<'a>(v: &'a Value, key: &str, tool: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{tool}: missing or non-string '{key}'"))
}

pub(super) fn parse_json_input(input: &str, tool: &str) -> Result<Value, String> {
    serde_json::from_str(input).map_err(|e| format!("{tool}: invalid JSON input ({e}): {input}"))
}

// ────── Wikipedia ────────────────────────────────────────────────────────

fn run_wikipedia_search(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "wikipedia_search")?;
    let query = extract_str(&v, "query", "wikipedia_search")?.to_string();

    let client = external_http_client()?;
    let resp = client
        .get("https://en.wikipedia.org/w/api.php")
        .query(&[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", query.as_str()),
            ("format", "json"),
            ("srlimit", "5"),
        ])
        .send()
        .map_err(|e| format!("wikipedia_search: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("wikipedia_search: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("wikipedia_search: parse failed: {e}"))?;

    let results: Vec<Value> = data
        .pointer("/query/search")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    let snippet = r
                        .get("snippet")
                        .and_then(Value::as_str)
                        .map(strip_html)
                        .unwrap_or_default();
                    json!({
                        "title": r.get("title").and_then(Value::as_str).unwrap_or(""),
                        "snippet": snippet,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_wikipedia_summary(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "wikipedia_summary")?;
    let title = extract_str(&v, "title", "wikipedia_summary")?;
    // Wikipedia REST API uses underscore-separated titles in the path.
    let encoded = title.replace(' ', "_");
    let url = format!("https://en.wikipedia.org/api/rest_v1/page/summary/{encoded}");

    let client = external_http_client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("wikipedia_summary: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("wikipedia_summary: no article titled '{title}'"));
    }
    if !status.is_success() {
        return Err(format!("wikipedia_summary: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("wikipedia_summary: parse failed: {e}"))?;

    Ok(json!({
        "title": data.get("title").and_then(Value::as_str).unwrap_or(title),
        "extract": data.get("extract").and_then(Value::as_str).unwrap_or(""),
        "url": data
            .pointer("/content_urls/desktop/page")
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
    .to_string())
}

// ────── Open-Meteo weather ───────────────────────────────────────────────

/// Geocode a free-text location into (lat, lon, display name) via Open-Meteo.
/// Accepts `"lat,lon"` shorthand for pre-resolved coordinates.
/// Translate Hebrew (and common transliterated) city names to their English
/// equivalents for the Open-Meteo geocoding API. Covers the ~30 most
/// populated Israeli cities plus a few common variants.
fn hebrew_city_alias(name: &str) -> Option<&'static str> {
    // Normalize: trim, lowercase for Latin comparisons.
    let trimmed = name.trim();
    match trimmed {
        // Hebrew script
        "ירושלים" => Some("Jerusalem"),
        "תל אביב" | "תל-אביב" | "תל אביב יפו" | "תל-אביב-יפו" => {
            Some("Tel Aviv")
        }
        "חיפה" => Some("Haifa"),
        "ראשון לציון" | "ראשון-לציון" => Some("Rishon LeZion"),
        "פתח תקווה" | "פתח-תקווה" | "פתח תקוה" => Some("Petah Tikva"),
        "אשדוד" => Some("Ashdod"),
        "נתניה" => Some("Netanya"),
        "באר שבע" | "באר-שבע" | "בארשבע" => Some("Beer Sheva"),
        "חולון" => Some("Holon"),
        "בני ברק" | "בני-ברק" => Some("Bnei Brak"),
        "רמת גן" | "רמת-גן" => Some("Ramat Gan"),
        "אשקלון" => Some("Ashkelon"),
        "רחובות" => Some("Rehovot"),
        "בת ים" | "בת-ים" => Some("Bat Yam"),
        "הרצליה" => Some("Herzliya"),
        "כפר סבא" | "כפר-סבא" => Some("Kfar Saba"),
        "חדרה" => Some("Hadera"),
        "מודיעין" | "מודיעין-מכבים-רעות" => Some("Modiin"),
        "לוד" => Some("Lod"),
        "רמלה" => Some("Ramla"),
        "נצרת" => Some("Nazareth"),
        "עכו" => Some("Acre"),
        "אילת" => Some("Eilat"),
        "טבריה" => Some("Tiberias"),
        "צפת" => Some("Safed"),
        "עפולה" => Some("Afula"),
        "קריית גת" | "קריית-גת" => Some("Kiryat Gat"),
        "נהריה" => Some("Nahariya"),
        "גבעתיים" => Some("Givatayim"),
        "רעננה" => Some("Raanana"),
        _ => {
            // Also handle common Latin transliterations that the API misses.
            match trimmed.to_lowercase().as_str() {
                "hedera" | "khadera" => Some("Hadera"),
                "beer sheva" | "beersheva" | "be'er sheva" => Some("Beer Sheva"),
                "petach tikva" | "petach-tikva" | "petah-tikva" => Some("Petah Tikva"),
                "rishon lezion" | "rishon-lezion" => Some("Rishon LeZion"),
                "bnei brak" | "bnei-brak" => Some("Bnei Brak"),
                "ramat-gan" => Some("Ramat Gan"),
                "kfar saba" | "kfar-saba" => Some("Kfar Saba"),
                "bat-yam" => Some("Bat Yam"),
                _ => None,
            }
        }
    }
}

fn resolve_location(location: &str) -> Result<(f64, f64, String), String> {
    let trimmed = location.trim();
    // Shortcut: accept "lat,lon" directly.
    if let Some((lat_s, lon_s)) = trimmed.split_once(',') {
        if let (Ok(lat), Ok(lon)) = (lat_s.trim().parse::<f64>(), lon_s.trim().parse::<f64>()) {
            return Ok((lat, lon, format!("{lat:.4},{lon:.4}")));
        }
    }

    // Translate Hebrew / transliterated city names before geocoding.
    let lookup_name = hebrew_city_alias(trimmed).unwrap_or(trimmed);

    let client = external_http_client()?;
    let resp = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", lookup_name),
            ("count", "1"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .map_err(|e| format!("geocoding: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("geocoding: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("geocoding: parse failed: {e}"))?;

    let first = data
        .pointer("/results/0")
        .ok_or_else(|| format!("geocoding: no match for '{location}'"))?;

    let lat = first
        .get("latitude")
        .and_then(Value::as_f64)
        .ok_or("geocoding: missing latitude")?;
    let lon = first
        .get("longitude")
        .and_then(Value::as_f64)
        .ok_or("geocoding: missing longitude")?;
    let name = first
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(trimmed)
        .to_string();
    let country = first
        .get("country")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let display = if country.is_empty() {
        name
    } else {
        format!("{name}, {country}")
    };
    Ok((lat, lon, display))
}

/// Convert a WMO weather code to a human label. Codes are documented at
/// <https://open-meteo.com/en/docs> — we only cover the common buckets so the
/// description stays short.
fn wmo_label(code: i64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 | 63 | 65 => "rain",
        66 | 67 => "freezing rain",
        71 | 73 | 75 => "snow",
        77 => "snow grains",
        80..=82 => "rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown",
    }
}

fn run_weather_current(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "weather_current")?;
    let location = extract_str(&v, "location", "weather_current")?;
    let (lat, lon, display) = resolve_location(location)?;

    let client = external_http_client()?;
    let resp = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string().as_str()),
            ("longitude", lon.to_string().as_str()),
            (
                "current",
                "temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m",
            ),
            ("timezone", "auto"),
            ("temperature_unit", "celsius"),
            ("wind_speed_unit", "kmh"),
        ])
        .send()
        .map_err(|e| format!("weather_current: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("weather_current: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("weather_current: parse failed: {e}"))?;

    let current = data
        .get("current")
        .ok_or("weather_current: response missing 'current'")?;
    let code = current
        .get("weather_code")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let temp = current.get("temperature_2m").and_then(Value::as_f64);
    let feels = current.get("apparent_temperature").and_then(Value::as_f64);
    let humidity = current.get("relative_humidity_2m").and_then(Value::as_f64);
    let wind = current.get("wind_speed_10m").and_then(Value::as_f64);
    let time = current.get("time").and_then(Value::as_str).unwrap_or("");

    Ok(json!({
        "location": display,
        "latitude": lat,
        "longitude": lon,
        "time": time,
        "condition": wmo_label(code),
        "weather_code": code,
        "temperature_c": temp,
        "feels_like_c": feels,
        "humidity_pct": humidity,
        "wind_kmh": wind,
    })
    .to_string())
}

fn run_weather_forecast(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "weather_forecast")?;
    let location = extract_str(&v, "location", "weather_forecast")?;
    let days = v
        .get("days")
        .and_then(Value::as_i64)
        .unwrap_or(3)
        .clamp(1, 7);
    let (lat, lon, display) = resolve_location(location)?;

    let client = external_http_client()?;
    let resp = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string().as_str()),
            ("longitude", lon.to_string().as_str()),
            (
                "daily",
                "weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum",
            ),
            ("timezone", "auto"),
            ("temperature_unit", "celsius"),
            ("forecast_days", days.to_string().as_str()),
        ])
        .send()
        .map_err(|e| format!("weather_forecast: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("weather_forecast: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("weather_forecast: parse failed: {e}"))?;

    let daily = data
        .get("daily")
        .ok_or("weather_forecast: response missing 'daily'")?;
    let dates = daily
        .get("time")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let codes = daily
        .get("weather_code")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let maxes = daily
        .get("temperature_2m_max")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mins = daily
        .get("temperature_2m_min")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let precips = daily
        .get("precipitation_sum")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let days_out: Vec<Value> = (0..dates.len())
        .map(|i| {
            let code = codes.get(i).and_then(Value::as_i64).unwrap_or(-1);
            json!({
                "date": dates.get(i).and_then(Value::as_str).unwrap_or(""),
                "condition": wmo_label(code),
                "weather_code": code,
                "max_c": maxes.get(i).and_then(Value::as_f64),
                "min_c": mins.get(i).and_then(Value::as_f64),
                "precipitation_mm": precips.get(i).and_then(Value::as_f64),
            })
        })
        .collect();

    Ok(json!({
        "location": display,
        "latitude": lat,
        "longitude": lon,
        "days": days_out,
    })
    .to_string())
}

// ────── GitHub ───────────────────────────────────────────────────────────

/// Build a GET request with GitHub auth headers already attached.
fn github_get(
    client: &reqwest::blocking::Client,
    url: &str,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

fn github_post(
    client: &reqwest::blocking::Client,
    url: &str,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

/// Fetch the authenticated user's `login`. Each call is one REST hit —
/// callers that need it multiple times in a turn should cache it, but for
/// single-tool-call paths this is fine.
fn github_me(client: &reqwest::blocking::Client, token: &str) -> Result<String, String> {
    let resp = github_get(client, "https://api.github.com/user", token)
        .send()
        .map_err(|e| format!("gh_me: request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("gh_me: HTTP {}", resp.status()));
    }
    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_me: parse failed: {e}"))?;
    data.get("login")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| "gh_me: response missing 'login'".to_string())
}

/// Shared helper for `gh_list_my_prs` and `gh_list_assigned_issues`.
fn github_search_issues(q: &str) -> Result<String, String> {
    let token = github_token()?;
    let client = external_http_client()?;
    let resp = github_get(&client, "https://api.github.com/search/issues", &token)
        .query(&[("q", q), ("per_page", "10"), ("sort", "updated")])
        .send()
        .map_err(|e| format!("gh_search_issues: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("gh_search_issues: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_search_issues: parse failed: {e}"))?;

    let items: Vec<Value> = data
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(10)
                .map(|i| {
                    json!({
                        "title": i.get("title").and_then(Value::as_str).unwrap_or(""),
                        "number": i.get("number").and_then(Value::as_i64).unwrap_or(0),
                        "state": i.get("state").and_then(Value::as_str).unwrap_or(""),
                        "url": i.get("html_url").and_then(Value::as_str).unwrap_or(""),
                        "repo": i.pointer("/repository_url").and_then(Value::as_str)
                            .and_then(|u| u.rsplit("/repos/").next())
                            .unwrap_or(""),
                        "updated_at": i.get("updated_at").and_then(Value::as_str).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "query": q,
        "count": items.len(),
        "items": items,
    })
    .to_string())
}

fn run_gh_list_my_prs() -> Result<String, String> {
    let token = github_token()?;
    let client = external_http_client()?;
    let me = github_me(&client, &token)?;
    let q = format!("is:pr author:{me} state:open");
    github_search_issues(&q)
}

fn run_gh_list_assigned_issues() -> Result<String, String> {
    let token = github_token()?;
    let client = external_http_client()?;
    let me = github_me(&client, &token)?;
    let q = format!("is:issue assignee:{me} state:open");
    github_search_issues(&q)
}

fn run_gh_get_issue(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_get_issue")?;
    let owner = extract_str(&v, "owner", "gh_get_issue")?;
    let repo = extract_str(&v, "repo", "gh_get_issue")?;
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .ok_or("gh_get_issue: missing 'number'")?;

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}");
    let resp = github_get(&client, &url, &token)
        .send()
        .map_err(|e| format!("gh_get_issue: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("gh_get_issue: {owner}/{repo}#{number} not found"));
    }
    if !status.is_success() {
        return Err(format!("gh_get_issue: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_get_issue: parse failed: {e}"))?;

    let body = data
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(2000)
        .collect::<String>();

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "title": data.get("title").and_then(Value::as_str).unwrap_or(""),
        "state": data.get("state").and_then(Value::as_str).unwrap_or(""),
        "author": data.pointer("/user/login").and_then(Value::as_str).unwrap_or(""),
        "body": body,
        "url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
        "is_pr": data.get("pull_request").is_some(),
        "labels": data.get("labels").and_then(Value::as_array).map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(Value::as_str).map(String::from))
                .collect::<Vec<_>>()
        }).unwrap_or_default(),
        "comments": data.get("comments").and_then(Value::as_i64).unwrap_or(0),
    })
    .to_string())
}

fn run_gh_create_issue(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_create_issue")?;
    let owner = extract_str(&v, "owner", "gh_create_issue")?;
    let repo = extract_str(&v, "repo", "gh_create_issue")?;
    let title = extract_str(&v, "title", "gh_create_issue")?;
    let body = v.get("body").and_then(Value::as_str).unwrap_or("");

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues");
    let payload = json!({ "title": title, "body": body });

    let resp = github_post(&client, &url, &token)
        .json(&payload)
        .send()
        .map_err(|e| format!("gh_create_issue: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "gh_create_issue: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_create_issue: parse failed: {e}"))?;

    Ok(json!({
        "ok": true,
        "number": data.get("number").and_then(Value::as_i64).unwrap_or(0),
        "url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
    })
    .to_string())
}

fn run_gh_comment_issue(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_comment_issue")?;
    let owner = extract_str(&v, "owner", "gh_comment_issue")?;
    let repo = extract_str(&v, "repo", "gh_comment_issue")?;
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .ok_or("gh_comment_issue: missing 'number'")?;
    let body = extract_str(&v, "body", "gh_comment_issue")?;

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments");
    let payload = json!({ "body": body });

    let resp = github_post(&client, &url, &token)
        .json(&payload)
        .send()
        .map_err(|e| format!("gh_comment_issue: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "gh_comment_issue: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_comment_issue: parse failed: {e}"))?;

    Ok(json!({
        "ok": true,
        "comment_url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
    })
    .to_string())
}

fn run_gh_search_code(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_search_code")?;
    let query = extract_str(&v, "query", "gh_search_code")?;

    let token = github_token()?;
    let client = external_http_client()?;
    let resp = github_get(&client, "https://api.github.com/search/code", &token)
        .query(&[("q", query), ("per_page", "5")])
        .send()
        .map_err(|e| format!("gh_search_code: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("gh_search_code: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_search_code: parse failed: {e}"))?;

    let items: Vec<Value> = data
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(5)
                .map(|i| {
                    json!({
                        "name": i.get("name").and_then(Value::as_str).unwrap_or(""),
                        "path": i.get("path").and_then(Value::as_str).unwrap_or(""),
                        "repo": i.pointer("/repository/full_name").and_then(Value::as_str).unwrap_or(""),
                        "url": i.get("html_url").and_then(Value::as_str).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "query": query,
        "count": items.len(),
        "results": items,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Sprint 9 Phase 0b — markets group (TradingView + vestige.fi)
//
// TradingView has no official public API — we hit the `scanner.tradingview.com`
// endpoints that the TradingView web screener itself uses. They're open (no
// auth) and widely consumed by community projects like `tradingview-ta`, but
// they sit in a ToS gray zone: TradingView's Terms explicitly prohibit
// "scraping" the website UI, while these JSON endpoints are served to any
// browser that loads the screener page. Practical risk at personal-agent
// volume is near-zero; do not cache-and-republish the data and keep a
// realistic User-Agent. Revisit if Claudette becomes multi-tenant.
//
// vestige.fi exposes a documented public REST API at api.vestige.fi — no
// key needed for reasonable read use. Canonical aggregator for Algorand DEX
// price data across Tinyman/Pact/Humble.
// ────────────────────────────────────────────────────────────────────────────

/// Map a `TradingView` interval string ("1m", "1h", "1d", "1W") to the suffix
/// the scanner API expects on column names like `Recommend.All|15`. Returns
/// an empty string for daily (the default, no suffix).
fn tv_interval_suffix(interval: &str) -> Result<&'static str, String> {
    match interval.trim() {
        "" | "1d" | "D" => Ok(""),
        "1m" => Ok("|1"),
        "5m" => Ok("|5"),
        "15m" => Ok("|15"),
        "30m" => Ok("|30"),
        "1h" | "60m" => Ok("|60"),
        "2h" | "120m" => Ok("|120"),
        "4h" | "240m" => Ok("|240"),
        "1W" | "W" => Ok("|1W"),
        "1M" | "M" => Ok("|1M"),
        other => Err(format!(
            "tv: unknown interval '{other}' — use 1m/5m/15m/30m/1h/4h/1d/1W/1M"
        )),
    }
}

/// Map a `TradingView` `Recommend.All` float score (−1.0 to 1.0) to a label.
/// Standard buckets used by the TV UI: ≥0.5 = `strong_buy`, ≥0.1 = buy, etc.
fn tv_rating_label(score: f64) -> &'static str {
    if score >= 0.5 {
        "strong_buy"
    } else if score >= 0.1 {
        "buy"
    } else if score > -0.1 {
        "neutral"
    } else if score > -0.5 {
        "sell"
    } else {
        "strong_sell"
    }
}

/// Normalise the user-visible market parameter to the path segment the
/// scanner API uses. Default: "america" (US stocks).
fn tv_market_path(market: Option<&str>) -> &'static str {
    match market.unwrap_or("america").to_lowercase().as_str() {
        "crypto" | "cryptos" => "crypto",
        "forex" | "fx" => "forex",
        "futures" | "futures_contracts" => "futures",
        _ => "america",
    }
}

/// Shared POST helper for `scanner.tradingview.com/{market}/scan`. Returns
/// the parsed `data` array (each entry is `{ s: symbol, d: [values...] }`).
fn tv_scan_request(market: &str, body: &Value) -> Result<Vec<Value>, String> {
    let url = format!("https://scanner.tradingview.com/{market}/scan");
    let client = external_http_client()?;
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .map_err(|e| format!("tv_scan: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "tv_scan: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_scan: parse failed: {e}"))?;

    Ok(data
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Try to resolve a bare or prefixed symbol to one that `TradingView` accepts.
///
/// If the symbol already contains a colon (e.g. `BINANCE:BTCUSDT`), use it
/// as-is. Otherwise, try a list of common exchange prefixes in order. Returns
/// `(resolved_symbol, rows)` where `rows` contains the scan result.
///
/// Common aliases handled:
///  - Crypto: `BTC` → `BINANCE:BTCUSDT`, `ETH` → `BINANCE:ETHUSDT`, etc.
///  - US stocks: bare ticker → `NASDAQ:<T>`, `NYSE:<T>`
fn resolve_tv_symbol(
    raw_symbol: &str,
    market: &str,
    columns: &Value,
) -> Result<(String, Vec<Value>), String> {
    let raw = raw_symbol.trim().to_uppercase();

    // Well-known commodity/index/crypto aliases. Covers the user's watchlist
    // and common names the model might use in natural language.
    let sym = match raw.as_str() {
        // ── Commodities (TVC: symbols work on any market path) ──
        "GOLD" | "XAU" | "XAUUSD" => "TVC:GOLD".to_string(),
        "SILVER" | "XAG" | "XAGUSD" => "TVC:SILVER".to_string(),
        "OIL" | "USOIL" | "CRUDE" | "WTI" | "CL" => "TVC:USOIL".to_string(),
        "BRENT" | "UKOIL" => "TVC:UKOIL".to_string(),
        "NATGAS" | "NG" | "NATURALGAS" => "TVC:NATURALGAS".to_string(),
        // ── Indices ──
        "NASDAQ" | "NDX" | "NDQ" | "QQQ" => "NASDAQ:NDX".to_string(),
        "SPX" | "SP500" | "S&P500" | "S&P" => "SP:SPX".to_string(),
        "DJI" | "DOW" | "DOWJONES" => "DJ:DJI".to_string(),
        "DXY" | "DOLLAR" => "TVC:DXY".to_string(),
        "VIX" => "TVC:VIX".to_string(),
        // ── Crypto (major pairs) ──
        "BTCUSD" | "BTCUSDT" => "BINANCE:BTCUSDT".to_string(),
        "ETHUSD" | "ETHUSDT" => "BINANCE:ETHUSDT".to_string(),
        "ALGOUSDT" | "ALGOUSI" | "ALGO" => "BINANCE:ALGOUSDT".to_string(),
        "KASUSDT" | "KAS" | "KASPA" => "MEXC:KASUSDT".to_string(),
        "ICPUSD" | "ICPUSDT" | "ICP" => "COINBASE:ICPUSD".to_string(),
        "QNTUSDT" | "QNT" => "BINANCE:QNTUSDT".to_string(),
        "BTCXAI" => "MEXC:BTCXAIUSDT".to_string(),
        // ── Forex ──
        "EURUSD" => "FX:EURUSD".to_string(),
        "USDJPY" => "FX:USDJPY".to_string(),
        "GBPUSD" => "FX:GBPUSD".to_string(),
        _ => raw,
    };

    // Already qualified (has exchange prefix) — try once.
    if sym.contains(':') {
        let body = json!({
            "symbols": { "tickers": [&sym], "query": { "types": [] } },
            "columns": columns,
        });
        let rows = tv_scan_request(market, &body)?;
        if !rows.is_empty() {
            return Ok((sym, rows));
        }
        return Err(format!("tv: symbol '{sym}' not found on market '{market}'"));
    }

    // Crypto shorthand: BTC → BINANCE:BTCUSDT, ETH → BINANCE:ETHUSDT, etc.
    static CRYPTO_SUFFIXES: &[(&str, &str)] = &[
        ("BINANCE:", "USDT"),
        ("COINBASE:", "USD"),
        ("BINANCE:", "USD"),
    ];

    // US stock exchanges.
    static STOCK_PREFIXES: &[&str] = &["NASDAQ:", "NYSE:", "AMEX:"];

    let candidates: Vec<String> = if market == "crypto" {
        CRYPTO_SUFFIXES
            .iter()
            .map(|(prefix, suffix)| format!("{prefix}{sym}{suffix}"))
            .collect()
    } else {
        STOCK_PREFIXES
            .iter()
            .map(|prefix| format!("{prefix}{sym}"))
            .collect()
    };

    for candidate in &candidates {
        let body = json!({
            "symbols": { "tickers": [candidate], "query": { "types": [] } },
            "columns": columns,
        });
        if let Ok(rows) = tv_scan_request(market, &body) {
            if !rows.is_empty() {
                return Ok((candidate.clone(), rows));
            }
        }
    }

    Err(format!(
        "tv: symbol '{raw_symbol}' not found. Tried: {candidates:?}. \
         Use tv_search_symbol to find the correct exchange prefix."
    ))
}

fn run_tv_get_quote(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_get_quote")?;
    let raw_symbol = extract_str(&v, "symbol", "tv_get_quote")?.to_string();
    let market = tv_market_path(v.get("market").and_then(Value::as_str));

    let columns = json!([
        "close",
        "change",
        "change_abs",
        "volume",
        "high",
        "low",
        "open"
    ]);
    let (symbol, rows) = resolve_tv_symbol(&raw_symbol, market, &columns)?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| format!("tv_get_quote: symbol '{symbol}' not found on market '{market}'"))?;

    let cells = row
        .get("d")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let get = |i: usize| cells.get(i).and_then(Value::as_f64);

    Ok(json!({
        "symbol": symbol,
        "market": market,
        "close": get(0),
        "change_pct": get(1),
        "change_abs": get(2),
        "volume": get(3),
        "high": get(4),
        "low": get(5),
        "open": get(6),
    })
    .to_string())
}

fn run_tv_technical_rating(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_technical_rating")?;
    let raw_symbol = extract_str(&v, "symbol", "tv_technical_rating")?.to_string();
    let interval = v.get("interval").and_then(Value::as_str).unwrap_or("1d");
    let suffix = tv_interval_suffix(interval)?;
    let market = tv_market_path(v.get("market").and_then(Value::as_str));

    // Build the three Recommend.* column names for the requested interval.
    let col_all = format!("Recommend.All{suffix}");
    let col_ma = format!("Recommend.MA{suffix}");
    let col_other = format!("Recommend.Other{suffix}");

    let columns = json!([col_all, col_ma, col_other]);
    let (symbol, rows) = resolve_tv_symbol(&raw_symbol, market, &columns)?;
    let row = rows.into_iter().next().ok_or_else(|| {
        format!("tv_technical_rating: symbol '{symbol}' not found on market '{market}'")
    })?;

    let cells = row
        .get("d")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let get = |i: usize| cells.get(i).and_then(Value::as_f64);
    let overall = get(0);
    let ma = get(1);
    let other = get(2);

    Ok(json!({
        "symbol": symbol,
        "market": market,
        "interval": interval,
        "overall_score": overall,
        "overall_rating": overall.map_or("", tv_rating_label),
        "moving_averages_score": ma,
        "moving_averages_rating": ma.map_or("", tv_rating_label),
        "oscillators_score": other,
        "oscillators_rating": other.map_or("", tv_rating_label),
    })
    .to_string())
}

fn run_tv_search_symbol(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_search_symbol")?;
    let query = extract_str(&v, "query", "tv_search_symbol")?;

    let client = external_http_client()?;
    let resp = client
        .get("https://symbol-search.tradingview.com/symbol_search/")
        .query(&[
            ("text", query),
            ("hl", "1"),
            ("exchange", ""),
            ("lang", "en"),
            ("type", ""),
            ("domain", "production"),
        ])
        .send()
        .map_err(|e| format!("tv_search_symbol: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("tv_search_symbol: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_search_symbol: parse failed: {e}"))?;

    // The endpoint returns either a top-level array or `{ "symbols": [...] }`
    // depending on the client; handle both shapes defensively.
    let raw = data
        .as_array()
        .cloned()
        .or_else(|| data.get("symbols").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    let results: Vec<Value> = raw
        .iter()
        .take(5)
        .map(|r| {
            let description = r
                .get("description")
                .and_then(Value::as_str)
                .map(strip_html)
                .unwrap_or_default();
            json!({
                "symbol": r.get("symbol").and_then(Value::as_str).unwrap_or(""),
                "description": description,
                "type": r.get("type").and_then(Value::as_str).unwrap_or(""),
                "exchange": r.get("exchange").and_then(Value::as_str).unwrap_or(""),
                "prefix": r.get("prefix").and_then(Value::as_str).unwrap_or(""),
                "country": r.get("country").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_tv_economic_calendar(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_economic_calendar")?;
    let days_ahead = v
        .get("days_ahead")
        .and_then(Value::as_i64)
        .unwrap_or(7)
        .clamp(1, 30);
    let countries = v
        .get("countries")
        .and_then(Value::as_str)
        .unwrap_or("US")
        .to_string();

    let now = chrono::Utc::now();
    let end = now + chrono::Duration::days(days_ahead);
    let from = now.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    let to = end.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();

    let client = external_http_client()?;
    // The economic-calendar subdomain enforces Origin/Referer checks at the
    // nginx layer — without these headers it returns 403 Forbidden. The
    // scanner subdomain does NOT enforce them, which is why scanner quotes
    // work but the calendar doesn't by default. Learned the hard way while
    // running the brain30_sprint9 test (2026-04-12).
    let resp = client
        .get("https://economic-calendar.tradingview.com/events")
        .query(&[
            ("from", from.as_str()),
            ("to", to.as_str()),
            ("countries", countries.as_str()),
        ])
        .header("Origin", "https://www.tradingview.com")
        .header("Referer", "https://www.tradingview.com/")
        .send()
        .map_err(|e| format!("tv_economic_calendar: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("tv_economic_calendar: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_economic_calendar: parse failed: {e}"))?;

    let events = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let results: Vec<Value> = events
        .iter()
        .take(20)
        .map(|e| {
            json!({
                "title": e.get("title").and_then(Value::as_str).unwrap_or(""),
                "country": e.get("country").and_then(Value::as_str).unwrap_or(""),
                "date": e.get("date").and_then(Value::as_str).unwrap_or(""),
                "importance": e.get("importance").and_then(Value::as_i64).unwrap_or(0),
                "actual": e.get("actual").and_then(Value::as_f64),
                "forecast": e.get("forecast").and_then(Value::as_f64),
                "previous": e.get("previous").and_then(Value::as_f64),
                "period": e.get("period").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "from": from,
        "to": to,
        "countries": countries,
        "count": results.len(),
        "events": results,
    })
    .to_string())
}

// ────── vestige.fi (Algorand ASAs) ───────────────────────────────────────
//
// The real API lives at `api.vestigelabs.org`, NOT `api.vestige.fi` (the
// latter DNS-resolves but returns Cloudflare 1016 / "origin unreachable" —
// discovered via browser DevTools on the vestige.fi homepage, 2026-04-12).
// The full OpenAPI spec is at `/openapi.json` — use that as the source of
// truth if this starts drifting.
//
// **Price denomination gotcha:** by default the API returns prices in ALGO
// (asset id 0). To get USD prices, pass `denominating_asset_id=31566704`
// (USDC). We denominate in USDC throughout so Claudette can report real USD
// numbers to the user without further conversion.

/// USDC ASA id on the Algorand mainnet. Used as the default denominating
/// asset so vestige prices come back in USD.
const VESTIGE_USDC_ASA_ID: i64 = 31566704;

/// Resolve the vestige.fi API base URL. Honours `VESTIGE_API_BASE` env var
/// so the user can flip to a paid tier or alternate endpoint if the free
/// base URL ever changes.
fn vestige_base_url() -> String {
    std::env::var("VESTIGE_API_BASE").unwrap_or_else(|_| "https://api.vestigelabs.org".to_string())
}

/// Extract the common `/assets/list` asset shape into the Claudette-facing
/// JSON format. Pulls price/volume/market-cap fields defensively so minor
/// upstream schema drift doesn't break the parse.
fn vestige_asset_json(a: &Value) -> Value {
    json!({
        "asa_id": a.get("id").and_then(Value::as_i64),
        "name": a.get("name").and_then(Value::as_str).unwrap_or(""),
        "ticker": a.get("ticker").and_then(Value::as_str).unwrap_or(""),
        "rank": a.get("rank").and_then(Value::as_i64),
        "price_usd": a.get("price").and_then(Value::as_f64),
        "price_24h_ago_usd": a.get("price1d").and_then(Value::as_f64),
        "price_7d_ago_usd": a.get("price7d").and_then(Value::as_f64),
        "change_24h_pct": calculate_change_pct(
            a.get("price").and_then(Value::as_f64),
            a.get("price1d").and_then(Value::as_f64),
        ),
        "volume_24h_usd": a.get("volume1d").and_then(Value::as_f64),
        "market_cap_usd": a.get("market_cap").and_then(Value::as_f64),
        "tvl_usd": a.get("tvl").and_then(Value::as_f64),
        "confidence": a.get("confidence").and_then(Value::as_f64),
        "total_supply": a.get("total_supply").and_then(Value::as_f64),
    })
}

/// Compute a percentage change from `old` to `new`, returning `None` if
/// either value is missing or `old` is zero.
fn calculate_change_pct(new: Option<f64>, old: Option<f64>) -> Option<f64> {
    let (n, o) = (new?, old?);
    if o.abs() < f64::EPSILON {
        return None;
    }
    Some((n - o) / o * 100.0)
}

fn run_vestige_asa_info(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_asa_info")?;
    let asa_id = v
        .get("asa_id")
        .and_then(Value::as_i64)
        .ok_or("vestige_asa_info: missing or non-numeric 'asa_id'")?;

    let base = vestige_base_url();
    // /assets/list with an asset_ids filter returns the same enriched shape
    // as the paginated listing (price, volume, market_cap, etc.), which is
    // what we want for a single-asset lookup.
    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let asa_id_str = asa_id.to_string();
    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/list"))
        .query(&[
            ("network_id", "0"),
            ("asset_ids", asa_id_str.as_str()),
            ("denominating_asset_id", denominating.as_str()),
        ])
        .send()
        .map_err(|e| format!("vestige_asa_info: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("vestige_asa_info: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_asa_info: parse failed: {e}"))?;

    let first = data
        .get("results")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .ok_or_else(|| format!("vestige_asa_info: no ASA with id {asa_id}"))?;

    Ok(vestige_asset_json(first).to_string())
}

fn run_vestige_search_asa(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_search_asa")?;
    let query = extract_str(&v, "query", "vestige_search_asa")?;
    let base = vestige_base_url();

    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/search"))
        .query(&[
            ("query", query),
            ("network_id", "0"),
            ("denominating_asset_id", denominating.as_str()),
            ("limit", "5"),
        ])
        .send()
        .map_err(|e| format!("vestige_search_asa: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("vestige_search_asa: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_search_asa: parse failed: {e}"))?;

    let raw = data
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let results: Vec<Value> = raw.iter().take(5).map(vestige_asset_json).collect();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_vestige_top_movers(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_top_movers")?;
    let direction = v
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("gainers")
        .to_lowercase();
    let limit = v
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(5)
        .clamp(1, 20);

    // Top movers by 24h price change. Vestige exposes price1d as "price
    // 24h ago" so we sort by current / price1d. Since there's no direct
    // sort param for "% change 24h", we sort by volume1d (which correlates
    // with movement) and compute change in post-processing. This trades a
    // little precision for a simpler call.
    let base = vestige_base_url();
    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let fetch_limit = (limit * 6).clamp(20, 100).to_string();

    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/list"))
        .query(&[
            ("network_id", "0"),
            ("denominating_asset_id", denominating.as_str()),
            ("order_by", "volume1d"),
            ("order_dir", "desc"),
            ("limit", fetch_limit.as_str()),
            // Filter out low-confidence spam tokens.
            ("tvl__gt", "10000"),
        ])
        .send()
        .map_err(|e| format!("vestige_top_movers: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("vestige_top_movers: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_top_movers: parse failed: {e}"))?;

    let raw = data
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Compute % change per asset, then sort.
    let mut scored: Vec<(f64, Value)> = raw
        .iter()
        .filter_map(|a| {
            let now = a.get("price").and_then(Value::as_f64)?;
            let then = a.get("price1d").and_then(Value::as_f64)?;
            if then.abs() < f64::EPSILON {
                return None;
            }
            let pct = (now - then) / then * 100.0;
            if !pct.is_finite() {
                return None;
            }
            Some((pct, a.clone()))
        })
        .collect();

    // Sort by % change: gainers → descending, losers → ascending.
    if direction == "losers" {
        scored.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        scored.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
    }

    let results: Vec<Value> = scored
        .into_iter()
        .take(limit as usize)
        .map(|(_, a)| vestige_asset_json(&a))
        .collect();

    Ok(json!({
        "direction": if direction == "losers" { "losers" } else { "gainers" },
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Sprint 10 — Telegram bot tools
// ────────────────────────────────────────────────────────────────────────────

/// Resolve the Telegram Bot API token via the unified secret store.
fn telegram_token() -> Result<String, String> {
    crate::secrets::read_secret("telegram").map_err(|_| {
        format!(
            "telegram: bot token not found. Message @BotFather on Telegram to create a bot, \
             then either export TELEGRAM_BOT_TOKEN or save it to {}",
            crate::secrets::secret_file_path("telegram").display()
        )
    })
}

/// Extract `chat_id` from a JSON value, accepting both string and number.
/// The model often passes `chat_id` as a number (e.g. `123456789`) rather
/// than a string, so we handle both.
fn tg_extract_chat_id(v: &Value, tool: &str) -> Result<String, String> {
    if let Some(s) = v.get("chat_id").and_then(Value::as_str) {
        return Ok(s.to_string());
    }
    if let Some(n) = v.get("chat_id").and_then(Value::as_i64) {
        return Ok(n.to_string());
    }
    Err(format!("{tool}: missing 'chat_id' (string or number)"))
}

/// Base URL for the Telegram Bot API.
fn tg_api_url(token: &str) -> String {
    format!("https://api.telegram.org/bot{token}")
}

/// `tg_send` — send a text message to a chat.
fn run_tg_send(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_send")?;
    // chat_id can be a string or number — the model often passes it as a number.
    let chat_id = tg_extract_chat_id(&v, "tg_send")?;
    let text = extract_str(&v, "text", "tg_send")?;

    let token = telegram_token()?;
    let client = external_http_client()?;
    let resp = client
        .post(format!("{}/sendMessage", tg_api_url(&token)))
        .json(&json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown",
        }))
        .send()
        .map_err(|e| format!("tg_send: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_send: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_send: parse failed: {e}"))?;

    let message_id = data
        .pointer("/result/message_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Ok(json!({
        "ok": true,
        "message_id": message_id,
        "chat_id": chat_id,
    })
    .to_string())
}

/// `tg_get_updates` — poll recent messages/commands sent to the bot.
fn run_tg_get_updates(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_get_updates")?;
    let limit = v
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .clamp(1, 100);
    let offset = v.get("offset").and_then(Value::as_i64);

    let token = telegram_token()?;
    let client = external_http_client()?;

    let mut params = vec![("limit", limit.to_string())];
    if let Some(off) = offset {
        params.push(("offset", off.to_string()));
    }

    let resp = client
        .get(format!("{}/getUpdates", tg_api_url(&token)))
        .query(&params)
        .send()
        .map_err(|e| format!("tg_get_updates: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_get_updates: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_get_updates: parse failed: {e}"))?;

    let updates = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Compact each update into a user-friendly shape.
    let results: Vec<Value> = updates
        .iter()
        .filter_map(|u| {
            let update_id = u.get("update_id").and_then(Value::as_i64)?;
            let msg = u.get("message")?;
            let from = msg
                .pointer("/from/first_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let username = msg
                .pointer("/from/username")
                .and_then(Value::as_str)
                .unwrap_or("");
            let chat_id = msg.pointer("/chat/id").and_then(Value::as_i64)?;
            let text = msg
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("[non-text message]");
            let date = msg.get("date").and_then(Value::as_i64).unwrap_or(0);
            Some(json!({
                "update_id": update_id,
                "chat_id": chat_id,
                "from": from,
                "username": username,
                "text": text,
                "date": date,
            }))
        })
        .collect();

    Ok(json!({
        "count": results.len(),
        "updates": results,
    })
    .to_string())
}

/// `tg_send_photo` — send a photo by URL to a chat.
fn run_tg_send_photo(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tg_send_photo")?;
    let chat_id = tg_extract_chat_id(&v, "tg_send_photo")?;
    let url = extract_str(&v, "url", "tg_send_photo")?;
    let caption = v.get("caption").and_then(Value::as_str).unwrap_or("");

    let token = telegram_token()?;
    let client = external_http_client()?;

    let mut body = json!({
        "chat_id": chat_id,
        "photo": url,
    });
    if !caption.is_empty() {
        body["caption"] = json!(caption);
        body["parse_mode"] = json!("Markdown");
    }

    let resp = client
        .post(format!("{}/sendPhoto", tg_api_url(&token)))
        .json(&body)
        .send()
        .map_err(|e| format!("tg_send_photo: request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tg_send_photo: HTTP error: {body}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tg_send_photo: parse failed: {e}"))?;

    let message_id = data
        .pointer("/result/message_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    Ok(json!({
        "ok": true,
        "message_id": message_id,
        "chat_id": chat_id,
    })
    .to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Sprint 9 Phase 0a — input validation for new tools. No network.

    #[test]
    fn wikipedia_search_rejects_missing_query() {
        let err = run_wikipedia_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn wikipedia_summary_rejects_missing_title() {
        let err = run_wikipedia_summary("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn weather_rejects_missing_location() {
        let err = run_weather_current("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
        let err = run_weather_forecast("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    // Registry-group tests (crate_info_rejects_missing_name,
    // npm_info_rejects_missing_name) live in src/tools/registry.rs.

    #[test]
    fn gh_get_issue_rejects_missing_fields() {
        let err = run_gh_get_issue(r#"{"owner":"example-org"}"#).unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    // ─── Sprint 9 Phase 0b — markets group ─────────────────────────

    #[test]
    fn tv_rating_label_buckets() {
        assert_eq!(tv_rating_label(0.8), "strong_buy");
        assert_eq!(tv_rating_label(0.3), "buy");
        assert_eq!(tv_rating_label(0.05), "neutral");
        assert_eq!(tv_rating_label(-0.05), "neutral");
        assert_eq!(tv_rating_label(-0.3), "sell");
        assert_eq!(tv_rating_label(-0.8), "strong_sell");
    }

    #[test]
    fn tv_interval_suffix_known() {
        assert_eq!(tv_interval_suffix("1d").unwrap(), "");
        assert_eq!(tv_interval_suffix("").unwrap(), "");
        assert_eq!(tv_interval_suffix("1m").unwrap(), "|1");
        assert_eq!(tv_interval_suffix("15m").unwrap(), "|15");
        assert_eq!(tv_interval_suffix("1h").unwrap(), "|60");
        assert_eq!(tv_interval_suffix("4h").unwrap(), "|240");
        assert_eq!(tv_interval_suffix("1W").unwrap(), "|1W");
        assert!(tv_interval_suffix("bogus").is_err());
    }

    #[test]
    fn tv_market_path_defaults_to_america() {
        assert_eq!(tv_market_path(None), "america");
        assert_eq!(tv_market_path(Some("america")), "america");
        assert_eq!(tv_market_path(Some("crypto")), "crypto");
        assert_eq!(tv_market_path(Some("CRYPTO")), "crypto");
        assert_eq!(tv_market_path(Some("forex")), "forex");
        assert_eq!(tv_market_path(Some("futures")), "futures");
        // Unknown falls back to america rather than erroring.
        assert_eq!(tv_market_path(Some("klingon")), "america");
    }

    #[test]
    fn tv_get_quote_rejects_missing_symbol() {
        let err = run_tv_get_quote("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn tv_technical_rating_rejects_missing_symbol() {
        let err = run_tv_technical_rating("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn tv_technical_rating_rejects_bad_interval() {
        let err =
            run_tv_technical_rating(r#"{"symbol":"NASDAQ:NVDA","interval":"nope"}"#).unwrap_err();
        assert!(err.contains("unknown interval"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_qualified_returns_as_is_on_failure() {
        // Qualified symbol (has colon) skips auto-resolution and gives a clear error.
        let err = resolve_tv_symbol("FAKE:NOSUCH", "america", &json!(["close"])).unwrap_err();
        assert!(err.contains("FAKE:NOSUCH"), "got: {err}");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_bare_crypto_tries_binance() {
        // Bare crypto symbol should try BINANCE:BTCUSDT etc. Won't succeed
        // without network, but the error should mention the candidates.
        let err = resolve_tv_symbol("FAKECOIN", "crypto", &json!(["close"])).unwrap_err();
        assert!(err.contains("BINANCE:FAKECOINUSDT"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_bare_stock_tries_nasdaq() {
        let err = resolve_tv_symbol("FAKESTOCK", "america", &json!(["close"])).unwrap_err();
        assert!(err.contains("NASDAQ:FAKESTOCK"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_commodity_aliases() {
        // Commodity aliases resolve to qualified symbols (which contain ':').
        // May succeed on the network (if the symbol is valid) or fail — either
        // way the resolved symbol should be the aliased one.
        match resolve_tv_symbol("GOLD", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("TVC:GOLD"), "got: {sym}"),
            Err(e) => assert!(e.contains("TVC:GOLD"), "got: {e}"),
        }
        match resolve_tv_symbol("OIL", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("TVC:USOIL"), "got: {sym}"),
            Err(e) => assert!(e.contains("TVC:USOIL"), "got: {e}"),
        }
        match resolve_tv_symbol("NASDAQ", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("NASDAQ:NDX"), "got: {sym}"),
            Err(e) => assert!(e.contains("NASDAQ:NDX"), "got: {e}"),
        }
    }

    #[test]
    fn tv_search_rejects_missing_query() {
        let err = run_tv_search_symbol("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn vestige_asa_info_rejects_missing_id() {
        let err = run_vestige_asa_info("{}").unwrap_err();
        assert!(err.contains("asa_id"), "got: {err}");
    }

    #[test]
    fn vestige_asa_info_rejects_non_numeric_id() {
        let err = run_vestige_asa_info(r#"{"asa_id":"USDC"}"#).unwrap_err();
        assert!(err.contains("asa_id"), "got: {err}");
    }

    #[test]
    fn vestige_search_rejects_missing_query() {
        let err = run_vestige_search_asa("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn vestige_base_url_default() {
        // Just check the default is stable. Env-var override test would
        // race with other tests so we skip it.
        let base = vestige_base_url();
        assert!(base.starts_with("http"));
        assert!(base.contains("vestige"));
    }

    #[test]
    fn github_token_env_lookup() {
        // Verify the error message mentions GITHUB_TOKEN and the file path
        // when the token is not set. If the user has GITHUB_TOKEN set in
        // their real env, the result is Ok and we skip the assertion.
        let result = github_token();
        if let Err(msg) = result {
            assert!(
                msg.contains("GITHUB_TOKEN"),
                "error should mention GITHUB_TOKEN: {msg}"
            );
            assert!(
                msg.contains("github.token"),
                "error should mention file path: {msg}"
            );
        }
    }

    #[test]
    fn wmo_label_known_codes() {
        assert_eq!(wmo_label(0), "clear");
        assert_eq!(wmo_label(3), "overcast");
        assert_eq!(wmo_label(63), "rain");
        assert_eq!(wmo_label(75), "snow");
        assert_eq!(wmo_label(95), "thunderstorm");
        assert_eq!(wmo_label(999), "unknown");
    }

    #[test]
    fn resolve_location_accepts_lat_lon_shorthand() {
        // "lat,lon" shortcut doesn't hit the network.
        let (lat, lon, display) = resolve_location("48.8566, 2.3522").unwrap();
        assert!((lat - 48.8566).abs() < 0.01);
        assert!((lon - 2.3522).abs() < 0.01);
        assert!(display.contains("48"));
    }

    #[test]
    fn hebrew_city_alias_resolves_hadera() {
        assert_eq!(hebrew_city_alias("חדרה"), Some("Hadera"));
    }

    #[test]
    fn hebrew_city_alias_resolves_tel_aviv() {
        assert_eq!(hebrew_city_alias("תל אביב"), Some("Tel Aviv"));
        assert_eq!(hebrew_city_alias("תל-אביב-יפו"), Some("Tel Aviv"));
    }

    #[test]
    fn hebrew_city_alias_resolves_latin_transliteration() {
        assert_eq!(hebrew_city_alias("hedera"), Some("Hadera"));
        assert_eq!(hebrew_city_alias("Hedera"), Some("Hadera"));
        assert_eq!(hebrew_city_alias("beer sheva"), Some("Beer Sheva"));
    }

    #[test]
    fn hebrew_city_alias_returns_none_for_english() {
        assert_eq!(hebrew_city_alias("London"), None);
        assert_eq!(hebrew_city_alias("New York"), None);
    }

    #[test]
    fn parse_json_input_reports_tool_name() {
        let err = parse_json_input("not json", "my_tool").unwrap_err();
        assert!(err.contains("my_tool"));
        assert!(err.contains("invalid JSON"));
    }

    #[test]
    fn extract_str_reports_missing_field() {
        let v: Value = json!({ "foo": 42 });
        let err = extract_str(&v, "bar", "my_tool").unwrap_err();
        assert!(err.contains("my_tool"));
        assert!(err.contains("bar"));
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Call mom tomorrow"), "call-mom-tomorrow");
        assert_eq!(slugify("  --weird///title!!!  "), "weird-title");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
    }

    #[test]
    fn normalize_path_collapses_dotdot() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_collapses_dot() {
        let p = normalize_path(Path::new("/a/./b/./c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_path_keeps_leading_dotdot_on_relative() {
        // Relative paths don't get to escape into nothing — leading .. stays.
        let p = normalize_path(Path::new("../foo"));
        assert_eq!(p, PathBuf::from("../foo"));
    }

    #[test]
    fn normalize_path_empty_becomes_dot() {
        let p = normalize_path(Path::new(""));
        assert_eq!(p, PathBuf::from("."));
    }

    #[test]
    fn expand_tilde_replaces_leading_tilde() {
        let home = user_home();
        assert_eq!(expand_tilde("~/foo/bar"), home.join("foo/bar"));
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn expand_tilde_leaves_other_paths_alone() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
        // Tilde not at start: shells leave it alone, so do we.
        assert_eq!(expand_tilde("foo/~/bar"), PathBuf::from("foo/~/bar"));
    }

    #[test]
    fn validate_read_path_accepts_paths_under_home() {
        // user_home itself should be valid; subdirs of it should be valid.
        let home = user_home();
        let target = home.join("some-doc.txt");
        let result = validate_read_path(target.to_str().unwrap());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn validate_read_path_rejects_traversal_escape() {
        // ~/.claudette/../../../etc/passwd resolves to outside home → reject
        let bad = "~/.claudette/../../../../../../etc/passwd";
        let result = validate_read_path(bad);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("restricted for safety"),
            "wrong error message"
        );
    }

    #[test]
    fn validate_write_path_accepts_scratch_subdirs() {
        let target = files_dir().join("draft.md");
        let result = validate_write_path(target.to_str().unwrap());
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    #[test]
    fn validate_write_path_rejects_outside_scratch() {
        // Even within home, anything outside ~/.claudette/files/ is rejected.
        let outside = user_home().join("Documents").join("draft.md");
        let result = validate_write_path(outside.to_str().unwrap());
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("sandboxed"),
            "wrong error message"
        );
    }

    #[test]
    fn validate_write_path_rejects_dotdot_escape_from_scratch() {
        // ~/.claudette/files/../../etc → outside scratch → reject
        let bad = "~/.claudette/files/../../etc/passwd";
        let result = validate_write_path(bad);
        assert!(result.is_err(), "expected reject, got {result:?}");
    }

    #[test]
    fn write_file_resolves_bare_relative_under_sandbox() {
        // Regression for the dolphins-post.txt bug: the model said
        // write_file("dolphins.txt", ...) and expected it to land in the
        // sandbox. Previously the path got resolved against CWD (typically
        // the workspace root) and the sandbox check rejected it. Now bare
        // relative paths are rooted at files_dir() so the model's intuition
        // works without it having to know the sandbox path.
        let target = files_dir().join("claudette-relative-test.txt");
        let _ = fs::remove_file(&target);

        let input = json!({
            "path": "claudette-relative-test.txt",
            "content": "wrote via bare relative path",
        })
        .to_string();
        let out = dispatch_tool("write_file", &input)
            .expect("relative write should succeed under sandbox");
        assert!(out.contains("\"ok\":true"), "got: {out}");
        assert!(target.exists(), "expected {} to exist", target.display());
        let content = fs::read_to_string(&target).unwrap();
        assert_eq!(content, "wrote via bare relative path");

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn write_file_still_rejects_absolute_outside_sandbox() {
        // Bare-relative resolution under the sandbox MUST NOT loosen the
        // sandbox check itself: an absolute path under the user's home but
        // outside ~/.claudette/files/ should still be rejected.
        let outside = user_home()
            .join("Documents")
            .join("definitely-not-allowed.txt");
        let input = json!({
            "path": outside.to_str().unwrap(),
            "content": "should be rejected",
        })
        .to_string();
        let result = dispatch_tool("write_file", &input);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(result.unwrap_err().contains("sandboxed"));
    }

    // ─── Sprint 13.3 — write_file refuses code extensions ───────────

    #[test]
    fn write_file_refuses_python_extension() {
        let input = json!({ "path": "user.py", "content": "x = 1\n" }).to_string();
        let err = dispatch_tool("write_file", &input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
        assert!(
            err.contains("generate_code"),
            "must mention generate_code: {err}"
        );
        // File must NOT have been written.
        assert!(!files_dir().join("user.py").exists());
    }

    #[test]
    fn write_file_refuses_rust_extension() {
        let input = json!({ "path": "lib.rs", "content": "fn main() {}\n" }).to_string();
        let err = dispatch_tool("write_file", &input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
    }

    #[test]
    fn write_file_refuses_uppercase_code_extension() {
        // Extension matching is case-insensitive.
        let input = json!({ "path": "App.HTML", "content": "<p>x</p>" }).to_string();
        let err = dispatch_tool("write_file", &input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
    }

    #[test]
    fn write_file_allows_text_extension() {
        let target = files_dir().join("write_refuse_allows_txt.txt");
        let _ = fs::remove_file(&target);
        let input = json!({
            "path": "write_refuse_allows_txt.txt",
            "content": "plain notes",
        })
        .to_string();
        let out = dispatch_tool("write_file", &input).expect(".txt should be allowed");
        assert!(out.contains("\"ok\":true"), "got: {out}");
        let _ = fs::remove_file(&target);
    }

    #[test]
    fn write_file_allows_data_and_config_extensions() {
        // JSON, MD, YAML, TOML — config/data formats stay on write_file.
        for (path, content) in [
            ("write_refuse_data.json", r#"{"k":"v"}"#),
            ("write_refuse_data.md", "# heading"),
            ("write_refuse_data.yaml", "k: v"),
            ("write_refuse_data.toml", "k = 'v'"),
        ] {
            let target = files_dir().join(path);
            let _ = fs::remove_file(&target);
            let input = json!({ "path": path, "content": content }).to_string();
            let out = dispatch_tool("write_file", &input)
                .unwrap_or_else(|e| panic!("{path} should be allowed, got: {e}"));
            assert!(out.contains("\"ok\":true"), "{path}: got {out}");
            let _ = fs::remove_file(&target);
        }
    }

    #[test]
    fn is_code_extension_classifies_correctly() {
        // Pure code → refuse.
        for ext in ["py", "rs", "js", "ts", "html", "css", "go", "sh"] {
            assert!(
                is_code_extension(&format!("file.{ext}")),
                "{ext} should be classified as code"
            );
        }
        // Config/data → allow.
        for ext in ["json", "toml", "yaml", "md", "txt", "xml", "ini"] {
            assert!(
                !is_code_extension(&format!("file.{ext}")),
                "{ext} should NOT be classified as code"
            );
        }
        // No extension → allow.
        assert!(!is_code_extension("README"));
    }

    #[test]
    fn read_file_round_trip_through_dispatch() {
        // Write a file via write_file then read it back via read_file, both
        // exercising the public dispatch entry point. Cleans up after itself.
        let path = files_dir().join("claudette-test-roundtrip.txt");
        let _ = fs::remove_file(&path);

        let write_input = json!({
            "path": path.to_str().unwrap(),
            "content": "hello from a unit test",
        })
        .to_string();
        let write_out =
            dispatch_tool("write_file", &write_input).expect("write_file should succeed");
        assert!(write_out.contains("\"ok\":true"));

        let read_input = json!({ "path": path.to_str().unwrap() }).to_string();
        let read_out = dispatch_tool("read_file", &read_input).expect("read_file should succeed");
        assert!(read_out.contains("hello from a unit test"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn get_capabilities_reports_real_config() {
        let raw = dispatch_tool("get_capabilities", "{}").expect("get_capabilities");
        let v: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(v["name"], "Claudette");
        // Sprint 8: tools are now reported as core + optional groups.
        let core = v["tools"]["core"].as_array().expect("core tools array");
        assert!(core.iter().any(|n| n == "get_capabilities"));
        assert!(core.iter().any(|n| n == "read_file"));
        assert!(core.iter().any(|n| n == "todo_add"));
        assert!(
            core.iter().any(|n| n == "enable_tools"),
            "enable_tools meta-tool must be in core"
        );

        // Optional groups should include git, ide, search, advanced.
        let groups = v["tools"]["optional_groups"]
            .as_array()
            .expect("optional_groups array");
        let group_names: Vec<&str> = groups
            .iter()
            .filter_map(|g| g.get("name").and_then(Value::as_str))
            .collect();
        assert!(group_names.contains(&"git"));
        assert!(group_names.contains(&"ide"));
        assert!(group_names.contains(&"search"));
        assert!(group_names.contains(&"advanced"));

        // Total count should add up.
        let total = v["tools"]["total"].as_u64().unwrap() as usize;
        let group_sum: usize = groups
            .iter()
            .map(|g| g["tools"].as_array().map_or(0, Vec::len))
            .sum();
        assert_eq!(total, core.len() + group_sum);

        // Context window must report the real (env-resolved) value, not a
        // made-up number.
        assert_eq!(
            v["context_window"]["num_ctx_tokens"].as_u64().unwrap(),
            u64::from(crate::api::current_num_ctx())
        );

        // Sandbox write boundary must be the actual files_dir(), not a guess.
        let write_path = v["sandbox"]["write"].as_str().unwrap();
        assert!(write_path.contains(".claudette"));
        assert!(write_path.ends_with("files"));
    }

    #[test]
    fn list_dir_classifies_file_and_subdir_correctly() {
        // Regression for the Windows reparse-point bug: build a temp dir
        // containing one real file and one real subdirectory, then verify
        // list_dir returns them with the correct `type` (not "unknown" or
        // mis-classified as "file").
        let tmp = std::env::temp_dir().join(format!(
            "claudette-test-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create tmp");
        fs::create_dir_all(tmp.join("subdir")).expect("create subdir");
        fs::write(tmp.join("hello.txt"), "hi").expect("write file");

        let input = json!({ "path": tmp.to_str().unwrap() }).to_string();
        let out = dispatch_tool("list_dir", &input).expect("list_dir should succeed");
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let entries = parsed["entries"].as_array().expect("entries array");

        let file_entry = entries
            .iter()
            .find(|e| e["name"] == "hello.txt")
            .expect("hello.txt should be listed");
        assert_eq!(file_entry["type"], "file", "hello.txt should be a file");
        assert_eq!(file_entry["size"], 2);

        let dir_entry = entries
            .iter()
            .find(|e| e["name"] == "subdir")
            .expect("subdir should be listed");
        assert_eq!(dir_entry["type"], "dir", "subdir should be a dir");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_dir_returns_known_entries() {
        // list the secretary's own data home — it must contain at least
        // notes/ and files/ once we've poked them.
        let _ = ensure_dir(&notes_dir());
        let _ = ensure_dir(&files_dir());

        let input = json!({ "path": claudette_home().to_str().unwrap() }).to_string();
        let out = dispatch_tool("list_dir", &input).expect("list_dir should succeed");
        assert!(out.contains("\"name\":\"files\""));
        assert!(out.contains("\"name\":\"notes\""));
    }

    // === glob_search tests ==================================================

    /// Build a unique temp directory under `~/.claudette/files/` and seed
    /// it with `seed` files. Caller must clean up.
    fn temp_seed_dir(label: &str, seed: &[(&str, &str)]) -> PathBuf {
        let dir = files_dir().join(format!(
            "test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create tmp");
        for (rel, content) in seed {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&p, content).expect("write seed file");
        }
        dir
    }

    #[test]
    fn glob_search_matches_files_under_home() {
        // Seed three files inside the sandbox; glob a recursive .txt match.
        let dir = temp_seed_dir(
            "glob",
            &[
                ("a.txt", "alpha"),
                ("nested/b.txt", "bravo"),
                ("nested/c.md", "charlie"),
            ],
        );
        let pattern = format!("{}/**/*.txt", dir.display());
        let input = json!({ "pattern": pattern }).to_string();
        let out = dispatch_tool("glob_search", &input).expect("glob_search should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        let count = v["count"].as_u64().unwrap();
        assert_eq!(count, 2, "expected 2 .txt matches, got {out}");
        let paths = v["paths"].as_array().unwrap();
        assert!(paths.iter().any(|p| p.as_str().unwrap().ends_with("a.txt")));
        assert!(paths.iter().any(|p| p.as_str().unwrap().ends_with("b.txt")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_search_rejects_path_outside_home() {
        // An absolute pattern under a system directory should be rejected
        // before we even invoke glob.
        let bad = if cfg!(windows) {
            "C:\\Windows\\**\\*.exe"
        } else {
            "/etc/**/*.conf"
        };
        let input = json!({ "pattern": bad }).to_string();
        let result = dispatch_tool("glob_search", &input);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(
            result.unwrap_err().contains("outside $HOME"),
            "wrong error message"
        );
    }

    #[test]
    fn glob_search_expands_tilde() {
        // ~/.claudette should resolve under home and find at least the
        // files/ directory we always create.
        let _ = ensure_dir(&files_dir());
        let input = json!({ "pattern": "~/.claudette/*" }).to_string();
        let out = dispatch_tool("glob_search", &input).expect("glob_search should succeed");
        assert!(out.contains(".claudette"));
    }

    // === grep_search tests ==================================================

    #[test]
    fn grep_search_finds_substring_match() {
        let dir = temp_seed_dir(
            "grep",
            &[
                ("notes.md", "TODO: write tests\nDONE: build tools\n"),
                ("other.txt", "nothing relevant here\n"),
            ],
        );
        let input = json!({
            "pattern": "todo",
            "path": dir.to_str().unwrap()
        })
        .to_string();
        let out = dispatch_tool("grep_search", &input).expect("grep_search should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["match_count"].as_u64().unwrap(), 1);
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches[0]["line"].as_u64().unwrap(), 1);
        assert!(matches[0]["text"].as_str().unwrap().contains("TODO"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_search_rejects_empty_pattern() {
        let input = json!({ "pattern": "", "path": "~" }).to_string();
        let result = dispatch_tool("grep_search", &input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pattern is empty"));
    }

    #[test]
    fn grep_search_rejects_path_outside_home() {
        let bad = if cfg!(windows) { "C:\\Windows" } else { "/etc" };
        let input = json!({ "pattern": "anything", "path": bad }).to_string();
        let result = dispatch_tool("grep_search", &input);
        assert!(result.is_err(), "expected reject, got {result:?}");
    }

    #[test]
    fn grep_search_skips_hidden_directories() {
        let dir = temp_seed_dir("grep-hidden", &[(".secret/inside.md", "FINDME")]);
        let input = json!({
            "pattern": "FINDME",
            "path": dir.to_str().unwrap()
        })
        .to_string();
        let out = dispatch_tool("grep_search", &input).expect("grep_search ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            0,
            "should skip hidden dir, got {out}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // === web_fetch / strip_html tests =======================================

    #[test]
    fn web_fetch_rejects_non_http_scheme() {
        let input = json!({ "url": "file:///etc/passwd" }).to_string();
        let result = dispatch_tool("web_fetch", &input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("http://"));

        let input = json!({ "url": "ftp://example.com" }).to_string();
        let result = dispatch_tool("web_fetch", &input);
        assert!(result.is_err());
    }

    #[test]
    fn strip_html_removes_simple_tags() {
        let html = "<p>Hello <strong>world</strong></p>";
        assert_eq!(strip_html(html), "Hello world");
    }

    #[test]
    fn strip_html_decodes_common_entities() {
        let html = "<p>2 &lt; 5 &amp;&amp; 5 &gt; 2</p>";
        assert_eq!(strip_html(html), "2 < 5 && 5 > 2");
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        let html = "<div>   lots\n\n\n  of    space   </div>";
        assert_eq!(strip_html(html), "lots of space");
    }

    #[test]
    fn strip_html_drops_script_and_style_blocks() {
        let html = "<html><head><style>body{color:red}</style></head>\
                    <body>visible<script>var x = 1;</script>also visible</body></html>";
        let cleaned = strip_html(html);
        assert!(cleaned.contains("visible"));
        assert!(cleaned.contains("also visible"));
        assert!(!cleaned.contains("color:red"), "style content leaked");
        assert!(!cleaned.contains("var x"), "script content leaked");
    }

    #[test]
    fn strip_html_handles_uppercase_script_tag() {
        let html = "before<SCRIPT>BAD</SCRIPT>after";
        let cleaned = strip_html(html);
        assert!(!cleaned.contains("BAD"));
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
    }

    // ── Sprint 10: Git tool upgrades ──────────────────────────────────

    #[test]
    fn extract_stat_number_from_summary() {
        let line = " 3 files changed, 45 insertions(+), 10 deletions(-)";
        assert_eq!(extract_stat_number(line, "insertion"), 45);
        assert_eq!(extract_stat_number(line, "deletion"), 10);
    }

    #[test]
    fn extract_stat_number_single_insertion() {
        let line = " 1 file changed, 1 insertion(+)";
        assert_eq!(extract_stat_number(line, "insertion"), 1);
        assert_eq!(extract_stat_number(line, "deletion"), 0);
    }

    #[test]
    fn extract_stat_number_missing() {
        assert_eq!(extract_stat_number("no match here", "insertion"), 0);
    }

    #[test]
    fn git_commit_empty_message_triggers_auto() {
        // With no staged changes, auto_commit_message should error
        // rather than producing an empty commit.
        let err = run_git_commit("{}");
        // This might fail because either: no git repo, or nothing staged.
        // Both are valid — we just need to confirm it doesn't succeed with
        // an empty message.
        if let Err(msg) = err {
            // Either "nothing staged" or git error — both acceptable.
            assert!(
                msg.contains("staged") || msg.contains("git"),
                "expected staged/git error, got: {msg}"
            );
        }
    }

    // ── Sprint 10: Telegram tool input validation ──────────────────────

    #[test]
    fn tg_send_rejects_missing_chat_id() {
        let err = run_tg_send(r#"{"text":"hello"}"#).unwrap_err();
        assert!(err.contains("chat_id"), "got: {err}");
    }

    #[test]
    fn tg_send_rejects_missing_text() {
        let err = run_tg_send(r#"{"chat_id":"123"}"#).unwrap_err();
        assert!(err.contains("text"), "got: {err}");
    }

    #[test]
    fn tg_send_photo_rejects_missing_url() {
        let err = run_tg_send_photo(r#"{"chat_id":"123"}"#).unwrap_err();
        assert!(err.contains("url"), "got: {err}");
    }

    #[test]
    fn tg_send_photo_rejects_missing_chat_id() {
        let err = run_tg_send_photo(r#"{"url":"https://example.com/img.jpg"}"#).unwrap_err();
        assert!(err.contains("chat_id"), "got: {err}");
    }

    #[test]
    fn telegram_token_error_mentions_botfather() {
        // If neither env var nor file is set, error should guide the user.
        let result = telegram_token();
        if let Err(msg) = result {
            assert!(msg.contains("BotFather"), "got: {msg}");
            assert!(msg.contains("telegram.token"), "got: {msg}");
        }
    }

    // ── Sprint 12 polish pass — time, notes, todos ──────────────────────────

    #[test]
    fn get_current_time_has_new_fields() {
        let out = run_get_current_time();
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        // Original fields still present.
        assert!(v["iso8601"].is_string());
        assert!(v["weekday"].is_string());
        assert!(v["date"].is_string());
        assert!(v["time"].is_string());
        assert!(v["timezone"].is_string());
        // New fields.
        assert!(v["weekday_num"].is_number(), "missing weekday_num");
        let dow = v["weekday_num"].as_u64().unwrap();
        assert!((1..=7).contains(&dow), "weekday_num out of range: {dow}");
        assert!(v["unix_timestamp"].is_number(), "missing unix_timestamp");
        assert!(v["human"].is_string(), "missing human");
        let human = v["human"].as_str().unwrap();
        assert!(
            human.contains(" at "),
            "human should contain ' at ': {human}"
        );
    }

    #[test]
    fn note_read_rejects_path_traversal() {
        let err = run_note_read(r#"{"id":"../secret.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_directory_separator() {
        let err = run_note_read(r#"{"id":"subdir/note.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_missing_id() {
        let err = run_note_read("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_nonexistent_note() {
        let err = run_note_read(r#"{"id":"9999-01-01T00-00-00-no-such-note.md"}"#).unwrap_err();
        assert!(err.contains("no note with id"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_path_traversal() {
        let err = run_note_delete(r#"{"id":"../boom.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_missing_id() {
        let err = run_note_delete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_nonexistent() {
        let err = run_note_delete(r#"{"id":"9999-01-01T00-00-00-no-such.md"}"#).unwrap_err();
        assert!(err.contains("no note with id"), "got: {err}");
    }

    #[test]
    fn todo_add_rejects_empty_text() {
        let err = run_todo_add(r#"{"text":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn todo_add_rejects_whitespace_only_text() {
        let err = run_todo_add(r#"{"text":"   "}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn todo_add_rejects_missing_text() {
        let err = run_todo_add("{}").unwrap_err();
        assert!(err.contains("missing 'text'"), "got: {err}");
    }

    #[test]
    fn todo_uncomplete_rejects_missing_id() {
        let err = run_todo_uncomplete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn todo_uncomplete_rejects_unknown_id() {
        let err = run_todo_uncomplete(r#"{"id":"t_does_not_exist_99999"}"#).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    #[test]
    fn todo_delete_rejects_missing_id() {
        let err = run_todo_delete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn todo_delete_rejects_unknown_id() {
        let err = run_todo_delete(r#"{"id":"t_does_not_exist_99999"}"#).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    #[test]
    fn core_tool_names_include_new_tools() {
        use crate::tool_groups::CORE_TOOL_NAMES;
        for tool in &["note_read", "note_delete", "todo_uncomplete", "todo_delete"] {
            assert!(
                CORE_TOOL_NAMES.contains(tool),
                "CORE_TOOL_NAMES missing {tool}"
            );
        }
    }

    #[test]
    fn note_list_accepts_limit_and_search_without_error() {
        // Parameters should be accepted even with no notes in the filesystem.
        let out = run_note_list(r#"{"limit":5,"search":"xyz-no-match"}"#).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["count"].is_number());
    }

    #[test]
    fn note_list_empty_tag_is_ignored() {
        // Regression: qwen3:8b sometimes sends `{"tag": ""}` or `{"tag": "   "}`
        // for a plain "list my notes". An empty filter must not exclude every
        // note — it must behave the same as no filter at all.
        let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let title = format!("__tag_empty_test_{stamp}");
        let create_out = run_note_create(
            &json!({ "title": title, "body": "x", "tags": "anything" }).to_string(),
        )
        .expect("note_create");
        let created: Value = serde_json::from_str(&create_out).unwrap();
        let note_id = created["id"].as_str().unwrap().to_string();

        for empty in ["", "   ", "\t"] {
            let out = run_note_list(&json!({ "tag": empty }).to_string()).expect("note_list");
            let v: Value = serde_json::from_str(&out).unwrap();
            assert!(
                v["count"].as_u64().unwrap() >= 1,
                "empty tag {empty:?} should not filter everything: {v}"
            );
            assert!(
                v.get("filtered_by_tag").is_none(),
                "empty tag should not report filtered_by_tag: {v}"
            );
        }

        // Cleanup.
        let _ = run_note_delete(&json!({ "id": note_id }).to_string());
    }

    #[test]
    fn todo_list_pending_only_flag_passes_through() {
        // Schema accepts pending_only: bool; result reflects it.
        let out = run_todo_list(r#"{"pending_only":true}"#).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["total"].is_number());
        assert!(v["pending"].is_number());
        assert_eq!(v["pending_only"], Value::Bool(true));
    }

    #[test]
    fn notes_and_todos_round_trip() {
        // End-to-end smoke test: create a note, read it back, delete it;
        // add a todo, complete, uncomplete, delete.
        // Uses unique titles/texts so it's safe alongside real data.
        let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let title = format!("__test_note_{stamp}");
        let body = format!("body-{stamp}");

        let create_input = json!({
            "title": title,
            "body": body,
            "tags": "test,polish"
        })
        .to_string();
        let create_out = run_note_create(&create_input).expect("note_create");
        let created: Value = serde_json::from_str(&create_out).unwrap();
        let note_id = created["id"].as_str().unwrap().to_string();

        // Read it back.
        let read_out = run_note_read(&json!({ "id": note_id }).to_string()).expect("note_read");
        let read: Value = serde_json::from_str(&read_out).unwrap();
        assert_eq!(read["title"], Value::String(title.clone()));
        assert!(read["body"].as_str().unwrap().contains(&body));
        assert_eq!(read["tags"], json!(["test", "polish"]));

        // list with search finds it.
        let list_out = run_note_list(&json!({ "search": title }).to_string()).expect("note_list");
        let list: Value = serde_json::from_str(&list_out).unwrap();
        assert!(list["count"].as_u64().unwrap() >= 1);

        // Delete it.
        let del_out = run_note_delete(&json!({ "id": note_id }).to_string()).expect("note_delete");
        assert!(del_out.contains("\"deleted\":true"));

        // ── todos ─────────────────────────────────────────────────────────
        let todo_text = format!("__test_todo_{stamp}");
        let add_out = run_todo_add(&json!({ "text": todo_text }).to_string()).expect("todo_add");
        let added: Value = serde_json::from_str(&add_out).unwrap();
        let todo_id = added["id"].as_str().unwrap().to_string();

        // Complete.
        let comp_out =
            run_todo_complete(&json!({ "id": todo_id }).to_string()).expect("todo_complete");
        assert!(comp_out.contains("\"done\":true"));

        // Uncomplete.
        let uncomp_out =
            run_todo_uncomplete(&json!({ "id": todo_id }).to_string()).expect("todo_uncomplete");
        assert!(uncomp_out.contains("\"done\":false"));

        // pending_only list should now include it.
        let list_out = run_todo_list(r#"{"pending_only":true}"#).expect("todo_list");
        assert!(list_out.contains(&todo_id));

        // Delete.
        let del_out = run_todo_delete(&json!({ "id": todo_id }).to_string()).expect("todo_delete");
        assert!(del_out.contains("\"deleted\":true"));

        // Confirm gone — second delete errors.
        let err = run_todo_delete(&json!({ "id": todo_id }).to_string()).unwrap_err();
        assert!(err.contains("no todo with id"), "got: {err}");
    }

    // ─── Sprint 13 — reference-file extraction ────────────────────────

    #[test]
    fn looks_like_path_recognises_common_shapes() {
        assert!(looks_like_path("~/foo/bar.py"));
        assert!(looks_like_path("~\\foo\\bar.py"));
        assert!(looks_like_path("./foo"));
        assert!(looks_like_path("../foo"));
        assert!(looks_like_path("/abs/path"));
        assert!(looks_like_path("C:\\Users\\me\\x.py"));
        assert!(looks_like_path("D:/dev/claudette/x.py"));
        assert!(!looks_like_path("plainword"));
        assert!(!looks_like_path("file.py")); // bare filename — not a path per se
        assert!(!looks_like_path("https://example.com/x.py"));
        assert!(!looks_like_path("http://example.com/x.py"));
    }

    #[test]
    fn has_code_extension_recognises_code_files() {
        assert!(has_code_extension("calculator.py"));
        assert!(has_code_extension("lib.RS")); // case-insensitive
        assert!(has_code_extension("path/to/file.ts"));
        assert!(!has_code_extension("no-extension"));
        assert!(!has_code_extension("readme"));
        // Extensions we don't include shouldn't leak in.
        assert!(!has_code_extension("archive.zip"));
    }

    #[test]
    fn extract_path_candidates_strips_punctuation_and_brackets() {
        let text = "Read the file ~/.claudette/files/calculator.py — it's a module.";
        let cands = extract_path_candidates(text);
        assert!(
            cands
                .iter()
                .any(|t| t == "~/.claudette/files/calculator.py"),
            "missing tilde path, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_keeps_bare_code_filename() {
        let cands = extract_path_candidates("Please read calculator.py carefully.");
        assert!(
            cands.iter().any(|t| t == "calculator.py"),
            "missing bare filename, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_ignores_urls_and_prose() {
        let cands =
            extract_path_candidates("Visit https://example.com/x.py then write a greeting.");
        // No URL, no plain prose words.
        assert!(
            !cands.iter().any(|t| t.contains("example.com")),
            "leaked URL: {cands:?}",
        );
        assert!(
            !cands.iter().any(|t| t == "greeting"),
            "kept prose word: {cands:?}",
        );
    }

    #[test]
    fn collect_reference_files_reads_tilde_path() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]); // start clean
                                        // Write a fixture under the user's home so validate_read_path accepts it.
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_fixture.py");
        let body = "class RefFixture:\n    def hello(self):\n        return 'hi'\n";
        fs::write(&fixture, body).unwrap();

        let desc =
            "Read the file ~/.claudette/files/refsprint_fixture.py and write tests for its API."
                .to_string();
        let refs = collect_reference_files(&[], &desc);

        // Cleanup before asserting so we don't leak fixtures on failure.
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("class RefFixture"),
            "content missing, got: {:?}",
            refs[0].content
        );
        assert_eq!(refs[0].path, "~/.claudette/files/refsprint_fixture.py");
    }

    #[test]
    fn collect_reference_files_ignores_missing_and_non_code() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // A description with a URL, a word, and a nonexistent filename.
        let desc = "Write a function. No file here. See http://example.com/foo.py and ghost.py.";
        let refs = collect_reference_files(&[], desc);
        assert!(
            refs.is_empty(),
            "expected no refs for missing files, got {refs:?}",
        );
    }

    #[test]
    fn collect_reference_files_caps_file_size() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_big_fixture.py");
        // 20 KB of Python, over the 16 KB per-file cap.
        let body: String = "x = 1\n".repeat(20 * 1024 / 6 + 1);
        fs::write(&fixture, &body).unwrap();

        let desc = "See ~/.claudette/files/refsprint_big_fixture.py".to_string();
        let refs = collect_reference_files(&[], &desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1);
        assert!(
            refs[0].content.contains("[truncated — file continues]"),
            "missing truncation marker",
        );
        assert!(
            refs[0].content.len() <= 16 * 1024 + 100,
            "content not truncated: {} bytes",
            refs[0].content.len()
        );
    }

    // ─── Sprint 13.1 — explicit reference_files param ────────────────

    #[test]
    fn collect_reference_files_uses_explicit_param() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_explicit_fixture.py");
        let body = "def explicit_marker():\n    return 'from explicit param'\n";
        fs::write(&fixture, body).unwrap();

        // Description has NO path tokens — only the explicit param does.
        let desc = "Write tests for the helper module.";
        let explicit = ["~/.claudette/files/refsprint_explicit_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("explicit_marker"),
            "content missing, got: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn collect_reference_files_dedups_explicit_and_scanner() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_dedup_fixture.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        // Same path appears in BOTH the explicit param and the description text.
        let desc = "Read ~/.claudette/files/refsprint_dedup_fixture.py and tests.";
        let explicit = ["~/.claudette/files/refsprint_dedup_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "duplicate not collapsed: {refs:?}");
    }

    #[test]
    fn collect_reference_files_silently_drops_invalid_explicit_paths() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // Explicit paths that don't exist on disk are filtered out, not erroring.
        let explicit = ["/this/path/does/not/exist.py", "~/no_such_file.py"];
        let refs = collect_reference_files(&explicit, "irrelevant description");
        assert!(refs.is_empty(), "expected empty, got {refs:?}");
    }

    // ─── Sprint 13.2 — per-turn user-prompt path stash ───────────────

    /// Serializer for any test that reads or writes `CURRENT_TURN_PATHS`.
    /// Cargo runs tests in parallel; without this guard, a stash-setting test
    /// can leak state into a stash-reading test running concurrently.
    static STASH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_stash() -> std::sync::MutexGuard<'static, ()> {
        // Recover from poisoning — a panic in one test must not block the rest.
        STASH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn extract_user_prompt_paths_keeps_existing_files_only() {
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_real.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        let prompt = "Add tests for ~/.claudette/files/refsprint_stash_real.py \
                      and also for ~/.claudette/files/refsprint_stash_ghost.py";
        let paths = extract_user_prompt_paths(prompt);
        let _ = fs::remove_file(&fixture);

        assert!(
            paths.iter().any(|p| p.contains("refsprint_stash_real.py")),
            "real path missing: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("refsprint_stash_ghost.py")),
            "ghost path leaked: {paths:?}"
        );
    }

    #[test]
    fn collect_reference_files_honours_turn_stash() {
        let _g = lock_stash();
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_fixture.py");
        let body = "def stash_marker():\n    return 'from turn stash'\n";
        fs::write(&fixture, body).unwrap();

        // Stash one path; pass empty explicit, irrelevant description.
        set_current_turn_paths(vec![
            "~/.claudette/files/refsprint_stash_fixture.py".to_string()
        ]);
        let refs = collect_reference_files(&[], "Write tests for the helper.");

        // Always clear the stash so other tests aren't affected.
        set_current_turn_paths(vec![]);
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "stash not honoured: {refs:?}");
        assert!(
            refs[0].content.contains("stash_marker"),
            "wrong content: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn set_current_turn_paths_overwrites_previous_stash() {
        let _g = lock_stash();
        set_current_turn_paths(vec!["a.py".to_string(), "b.py".to_string()]);
        assert_eq!(current_turn_paths().len(), 2);
        set_current_turn_paths(vec!["c.py".to_string()]);
        assert_eq!(current_turn_paths(), vec!["c.py".to_string()]);
        set_current_turn_paths(vec![]);
        assert!(current_turn_paths().is_empty());
    }

    #[test]
    fn collect_reference_files_explicit_respects_max_files() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let mut fixtures = Vec::new();
        let mut explicit_paths = Vec::new();
        for i in 0..6 {
            let p = dir.join(format!("refsprint_cap_fixture_{i}.py"));
            fs::write(&p, format!("# fixture {i}\nx = {i}\n")).unwrap();
            fixtures.push(p);
            explicit_paths.push(format!("~/.claudette/files/refsprint_cap_fixture_{i}.py"));
        }
        let explicit_refs: Vec<&str> = explicit_paths.iter().map(String::as_str).collect();

        let refs = collect_reference_files(&explicit_refs, "");

        for f in &fixtures {
            let _ = fs::remove_file(f);
        }

        assert_eq!(
            refs.len(),
            REF_MAX_FILES,
            "expected cap, got {}",
            refs.len()
        );
    }
}
