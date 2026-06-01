//! GitHub group — REST API tools. Token comes from
//! `crate::secrets::read_secret("github")` (GITHUB_TOKEN env or
//! `~/.claudette/secrets/github.token`).
//!
//! Tools split into two flavours:
//! - User-account scoped: `gh_inbox(scope)` — `scope="my_prs"` lists open
//!   PRs you authored, `scope="assigned"` lists open issues assigned to
//!   you, `scope="repo_issues"` (with owner+repo) lists issues in any
//!   repo. v0.6.0 collapsed the old `gh_list_my_prs` and
//!   `gh_list_assigned_issues` into this polymorphic entry.
//! - Repo-scoped (the brownfield set): `gh_get_issue`, `gh_create_issue`,
//!   `gh_comment_issue`, `gh_search_code`, `gh_list_repo_issues`,
//!   `gh_pr_status`, `gh_fork`, `gh_create_pr`. `gh_list_repo_issues`
//!   stays advertised for now — the dedicated repo+owner shape is
//!   friendlier than reaching for `gh_inbox(scope="repo_issues", ...)`.
//!
//! Self-contained: all helpers (`github_token`, `github_get`, `github_post`,
//! `github_me`, `github_search_issues`) are private to this module.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input, wrap_untrusted};

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "gh_inbox",
                "description": "List GitHub inbox items by scope. scope='my_prs' (open PRs I authored), 'assigned' (issues assigned to me), or 'repo_issues' (open issues in any repo — pass owner+repo). Requires GITHUB_TOKEN.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "scope": { "type": "string", "description": "'my_prs', 'assigned', or 'repo_issues'" },
                        "owner": { "type": "string", "description": "Repo owner (required for scope='repo_issues')" },
                        "repo":  { "type": "string", "description": "Repo name (required for scope='repo_issues')" }
                    },
                    "required": ["scope"]
                }
            }
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_list_repo_issues",
                "description": "List open issues in any repo (PRs filtered out). Use to browse a brownfield target.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":  { "type": "string", "description": "Repo owner" },
                        "repo":   { "type": "string", "description": "Repo name" },
                        "state":  { "type": "string", "description": "open|closed|all (default: open)" },
                        "labels": { "type": "string", "description": "Comma-separated label filter (optional)" },
                        "limit":  { "type": "number", "description": "Max items, capped at 30 (default: 10)" }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_pr_status",
                "description": "Fetch a PR's mergeable / draft / checks state by owner/repo/number. (v0.6.0: prefer gh_pr_view for a richer single-shot snapshot.)",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":  { "type": "string", "description": "Repo owner" },
                        "repo":   { "type": "string", "description": "Repo name" },
                        "number": { "type": "number", "description": "PR number" }
                    },
                    "required": ["owner", "repo", "number"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_pr_view",
                "description": "Single-shot PR snapshot: body + truncated diff + last 20 review/issue comments + check-runs summary. Use this for 'show me PR #N' style questions. Folds the gh_pr_status surface.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":        { "type": "string", "description": "Repo owner" },
                        "repo":         { "type": "string", "description": "Repo name" },
                        "number":       { "type": "number", "description": "PR number" },
                        "include_diff": { "type": "boolean", "description": "Include the diff (truncated to 30k chars). Default true." }
                    },
                    "required": ["owner", "repo", "number"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_workflow_logs",
                "description": "Fetch failed-job log lines for a GitHub Actions run. Provide owner+repo plus one of: `pr` (auto-resolve to the latest failed run on the PR's head sha), `run_id`, or `job_id`. Returns matching lines around error/FAILED/panic markers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner":  { "type": "string", "description": "Repo owner" },
                        "repo":   { "type": "string", "description": "Repo name" },
                        "pr":     { "type": "number", "description": "PR number — auto-resolves to the latest failed workflow run on the PR's head sha." },
                        "run_id": { "type": "number", "description": "Explicit workflow run id." },
                        "job_id": { "type": "number", "description": "Explicit job id (skips the run-lookup hop)." }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_fork",
                "description": "Fork a repo to the authenticated user's account. Required before pushing fix branches when you can't push to upstream.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string", "description": "Source repo owner" },
                        "repo":  { "type": "string", "description": "Source repo name" }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_create_pr",
                "description": "Open a pull request. head is `branch` for same-repo or `username:branch` for fork PRs. Run git_push first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string", "description": "Target repo owner (the upstream)" },
                        "repo":  { "type": "string", "description": "Target repo name" },
                        "title": { "type": "string", "description": "PR title" },
                        "body":  { "type": "string", "description": "PR body (Markdown). Use 'Fixes #N' to auto-close issues." },
                        "head":  { "type": "string", "description": "Source ref: 'branch' (same repo) or 'username:branch' (fork)" },
                        "base":  { "type": "string", "description": "Target branch (e.g. 'main', 'master')" },
                        "draft": { "type": "boolean", "description": "Open as draft (default: false)" }
                    },
                    "required": ["owner", "repo", "title", "head", "base"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "gh_inbox" => run_gh_inbox(input),
        "gh_get_issue" => run_gh_get_issue(input),
        "gh_create_issue" => run_gh_create_issue(input),
        "gh_comment_issue" => run_gh_comment_issue(input),
        "gh_search_code" => run_gh_search_code(input),
        "gh_list_repo_issues" => run_gh_list_repo_issues(input),
        "gh_pr_status" => run_gh_pr_status(input),
        "gh_pr_view" => run_gh_pr_view(input),
        "gh_workflow_logs" => run_gh_workflow_logs(input),
        "gh_fork" => run_gh_fork(input),
        "gh_create_pr" => run_gh_create_pr(input),
        _ => return None,
    };
    Some(result)
}

/// `gh_inbox(scope, owner?, repo?)` — polymorphic GitHub inbox. Routes to
/// the same backend search as the legacy `gh_list_my_prs` /
/// `gh_list_assigned_issues` (for the user-scoped scopes), or to
/// `run_gh_list_repo_issues` (for scope='repo_issues').
fn run_gh_inbox(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_inbox")?;
    let scope = extract_str(&v, "scope", "gh_inbox")?;
    match scope {
        "my_prs" => run_gh_list_my_prs(),
        "assigned" => run_gh_list_assigned_issues(),
        "repo_issues" => {
            // owner+repo are required for this scope; let the existing
            // run_gh_list_repo_issues do the validation by forwarding the
            // input payload unchanged (it already requires those fields).
            run_gh_list_repo_issues(input)
        }
        other => Err(format!(
            "gh_inbox: unknown scope '{other}' — use 'my_prs', 'assigned', or 'repo_issues'"
        )),
    }
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

    // The issue body is attacker-controlled Markdown — wrap in
    // <untrusted> and defang close-tag injection. Same defense as
    // web_fetch and Gmail. Title is short + low-signal for injection
    // so we leave it bare; if that changes, wrap it too.
    let wrapped_body = wrap_untrusted(&format!("github-issue:{owner}/{repo}#{number}"), &body);

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "title": data.get("title").and_then(Value::as_str).unwrap_or(""),
        "state": data.get("state").and_then(Value::as_str).unwrap_or(""),
        "author": data.pointer("/user/login").and_then(Value::as_str).unwrap_or(""),
        "body": wrapped_body,
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

fn run_gh_list_repo_issues(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_list_repo_issues")?;
    let owner = extract_str(&v, "owner", "gh_list_repo_issues")?;
    let repo = extract_str(&v, "repo", "gh_list_repo_issues")?;
    let state = v.get("state").and_then(Value::as_str).unwrap_or("open");
    let labels = v.get("labels").and_then(Value::as_str).unwrap_or("");
    let limit = v
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 30);

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues");
    let per_page = limit.to_string();
    let mut query: Vec<(&str, &str)> = vec![
        ("state", state),
        ("per_page", &per_page),
        ("sort", "updated"),
    ];
    if !labels.is_empty() {
        query.push(("labels", labels));
    }

    let resp = github_get(&client, &url, &token)
        .query(&query)
        .send()
        .map_err(|e| format!("gh_list_repo_issues: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("gh_list_repo_issues: {owner}/{repo} not found"));
    }
    if !status.is_success() {
        return Err(format!("gh_list_repo_issues: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_list_repo_issues: parse failed: {e}"))?;

    let items: Vec<Value> = data
        .as_array()
        .map(|arr| {
            arr.iter()
                // Filter out PRs — the issues endpoint returns both.
                .filter(|i| i.get("pull_request").is_none())
                .map(|i| {
                    json!({
                        "number": i.get("number").and_then(Value::as_i64).unwrap_or(0),
                        "title": i.get("title").and_then(Value::as_str).unwrap_or(""),
                        "state": i.get("state").and_then(Value::as_str).unwrap_or(""),
                        "author": i.pointer("/user/login").and_then(Value::as_str).unwrap_or(""),
                        "url": i.get("html_url").and_then(Value::as_str).unwrap_or(""),
                        "labels": i.get("labels").and_then(Value::as_array).map(|arr| {
                            arr.iter()
                                .filter_map(|l| l.get("name").and_then(Value::as_str).map(String::from))
                                .collect::<Vec<_>>()
                        }).unwrap_or_default(),
                        "comments": i.get("comments").and_then(Value::as_i64).unwrap_or(0),
                        "updated_at": i.get("updated_at").and_then(Value::as_str).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "state": state,
        "count": items.len(),
        "items": items,
    })
    .to_string())
}

fn run_gh_pr_status(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_pr_status")?;
    let owner = extract_str(&v, "owner", "gh_pr_status")?;
    let repo = extract_str(&v, "repo", "gh_pr_status")?;
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .ok_or("gh_pr_status: missing 'number'")?;

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}");
    let resp = github_get(&client, &url, &token)
        .send()
        .map_err(|e| format!("gh_pr_status: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("gh_pr_status: {owner}/{repo}#{number} not found"));
    }
    if !status.is_success() {
        return Err(format!("gh_pr_status: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_pr_status: parse failed: {e}"))?;

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "state": data.get("state").and_then(Value::as_str).unwrap_or(""),
        "draft": data.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "merged": data.get("merged").and_then(Value::as_bool).unwrap_or(false),
        // mergeable can be null while GitHub computes it — surface that as "unknown".
        "mergeable": data.get("mergeable").and_then(Value::as_bool),
        "mergeable_state": data.get("mergeable_state").and_then(Value::as_str).unwrap_or(""),
        "head_ref": data.pointer("/head/ref").and_then(Value::as_str).unwrap_or(""),
        "head_sha": data.pointer("/head/sha").and_then(Value::as_str).unwrap_or(""),
        "base_ref": data.pointer("/base/ref").and_then(Value::as_str).unwrap_or(""),
        "url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
    })
    .to_string())
}

/// `gh_pr_view` — v0.6.0 single-shot PR snapshot. Pulls PR data + diff +
/// last 20 issue-comments + check-runs summary in one tool call. Folds
/// the gh_pr_status use case (everything that tool returned is in here
/// too, just under different keys for clarity).
fn run_gh_pr_view(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_pr_view")?;
    let owner = extract_str(&v, "owner", "gh_pr_view")?;
    let repo = extract_str(&v, "repo", "gh_pr_view")?;
    let number = v
        .get("number")
        .and_then(Value::as_i64)
        .ok_or("gh_pr_view: missing 'number'")?;
    let include_diff = v
        .get("include_diff")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let token = github_token()?;
    let client = external_http_client()?;

    // PR metadata (same shape as gh_pr_status, plus body).
    let pr_url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}");
    let pr_resp = github_get(&client, &pr_url, &token)
        .send()
        .map_err(|e| format!("gh_pr_view: PR request failed: {e}"))?;
    let pr_status = pr_resp.status();
    if pr_status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("gh_pr_view: {owner}/{repo}#{number} not found"));
    }
    if !pr_status.is_success() {
        return Err(format!("gh_pr_view: PR HTTP {pr_status}"));
    }
    let pr: Value = pr_resp
        .json()
        .map_err(|e| format!("gh_pr_view: PR parse failed: {e}"))?;

    let head_sha = pr
        .pointer("/head/sha")
        .and_then(Value::as_str)
        .unwrap_or("");

    // PR body is attacker-controlled — wrap in <untrusted>.
    let body_raw = pr
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(4000)
        .collect::<String>();
    let body = wrap_untrusted(
        &format!("github-pr-body:{owner}/{repo}#{number}"),
        &body_raw,
    );

    // Diff — separate request with a different Accept header.
    let diff = if include_diff {
        let diff_resp = client
            .get(&pr_url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github.v3.diff")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .ok();
        diff_resp
            .and_then(|r| {
                if r.status().is_success() {
                    r.text().ok()
                } else {
                    None
                }
            })
            .map(|s| s.chars().take(30_000).collect::<String>())
            .unwrap_or_default()
    } else {
        String::new()
    };

    // Recent issue comments — capped to last 20. Each is attacker-
    // controlled, so wrap in <untrusted> per-comment.
    let comments_url =
        format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}/comments");
    let comments_resp = github_get(&client, &comments_url, &token)
        .query(&[
            ("per_page", "20"),
            ("sort", "created"),
            ("direction", "desc"),
        ])
        .send()
        .map_err(|e| format!("gh_pr_view: comments request failed: {e}"))?;
    let comments_raw: Vec<Value> = if comments_resp.status().is_success() {
        comments_resp
            .json::<Value>()
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let comments: Vec<Value> = comments_raw
        .iter()
        .map(|c| {
            let author = c
                .pointer("/user/login")
                .and_then(Value::as_str)
                .unwrap_or("");
            let raw_body = c
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(800)
                .collect::<String>();
            let wrapped = wrap_untrusted(&format!("github-comment:{author}"), &raw_body);
            json!({
                "author": author,
                "created_at": c.get("created_at").and_then(Value::as_str).unwrap_or(""),
                "body": wrapped,
            })
        })
        .collect();

    // Check-runs for the head commit. Summarises pass/fail/in_progress
    // counts with a per-failed-job list.
    let mut checks_summary = json!(null);
    if !head_sha.is_empty() {
        let checks_url =
            format!("https://api.github.com/repos/{owner}/{repo}/commits/{head_sha}/check-runs");
        if let Ok(check_resp) = github_get(&client, &checks_url, &token).send() {
            if check_resp.status().is_success() {
                if let Ok(check_data) = check_resp.json::<Value>() {
                    let runs = check_data
                        .get("check_runs")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut success = 0u32;
                    let mut failed = 0u32;
                    let mut in_progress = 0u32;
                    let mut neutral = 0u32;
                    let mut failed_runs: Vec<Value> = Vec::new();
                    for run in &runs {
                        let status = run.get("status").and_then(Value::as_str).unwrap_or("");
                        let conclusion =
                            run.get("conclusion").and_then(Value::as_str).unwrap_or("");
                        if status != "completed" {
                            in_progress += 1;
                            continue;
                        }
                        match conclusion {
                            "success" => success += 1,
                            "failure" | "timed_out" | "cancelled" | "action_required" => {
                                failed += 1;
                                failed_runs.push(json!({
                                    "name": run.get("name").and_then(Value::as_str).unwrap_or(""),
                                    "conclusion": conclusion,
                                    "url": run.get("html_url").and_then(Value::as_str).unwrap_or(""),
                                }));
                            }
                            "neutral" | "skipped" => neutral += 1,
                            _ => {}
                        }
                    }
                    checks_summary = json!({
                        "total": runs.len(),
                        "success": success,
                        "failed": failed,
                        "in_progress": in_progress,
                        "neutral_or_skipped": neutral,
                        "failed_runs": failed_runs,
                    });
                }
            }
        }
    }

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "title": pr.get("title").and_then(Value::as_str).unwrap_or(""),
        "author": pr.pointer("/user/login").and_then(Value::as_str).unwrap_or(""),
        "state": pr.get("state").and_then(Value::as_str).unwrap_or(""),
        "draft": pr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "merged": pr.get("merged").and_then(Value::as_bool).unwrap_or(false),
        "mergeable": pr.get("mergeable").and_then(Value::as_bool),
        "mergeable_state": pr.get("mergeable_state").and_then(Value::as_str).unwrap_or(""),
        "head_ref": pr.pointer("/head/ref").and_then(Value::as_str).unwrap_or(""),
        "head_sha": head_sha,
        "base_ref": pr.pointer("/base/ref").and_then(Value::as_str).unwrap_or(""),
        "url": pr.get("html_url").and_then(Value::as_str).unwrap_or(""),
        "body": body,
        "diff": diff,
        "diff_included": include_diff,
        "comments": comments,
        "checks": checks_summary,
    })
    .to_string())
}

/// `gh_workflow_logs` — auto-extract failed-job error context. The brain
/// gets to ask "what broke?" without paging through the GitHub UI.
///
/// Resolution order:
/// 1. `job_id` → fetch that job's log directly.
/// 2. `run_id` → list jobs, pick failed ones, fetch each log.
/// 3. `pr` → look up PR head_sha → find latest failed run on that sha →
///    list its failed jobs → fetch each log.
fn run_gh_workflow_logs(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_workflow_logs")?;
    let owner = extract_str(&v, "owner", "gh_workflow_logs")?;
    let repo = extract_str(&v, "repo", "gh_workflow_logs")?;
    let pr = v.get("pr").and_then(Value::as_i64);
    let explicit_run = v.get("run_id").and_then(Value::as_i64);
    let explicit_job = v.get("job_id").and_then(Value::as_i64);

    if pr.is_none() && explicit_run.is_none() && explicit_job.is_none() {
        return Err("gh_workflow_logs: provide one of 'pr', 'run_id', or 'job_id'".to_string());
    }

    let token = github_token()?;
    let client = external_http_client()?;

    let mut sections: Vec<Value> = Vec::new();

    if let Some(job_id) = explicit_job {
        let log = fetch_job_log(&client, &token, owner, repo, job_id)?;
        sections.push(json!({
            "job_id": job_id,
            "lines": extract_error_lines(&log),
        }));
    } else {
        // Resolve to a run id.
        let run_id = if let Some(rid) = explicit_run {
            rid
        } else if let Some(pr_num) = pr {
            resolve_run_from_pr(&client, &token, owner, repo, pr_num)?
        } else {
            unreachable!("guarded above")
        };
        let jobs = list_failed_jobs(&client, &token, owner, repo, run_id)?;
        if jobs.is_empty() {
            return Ok(json!({
                "owner": owner,
                "repo": repo,
                "run_id": run_id,
                "note": "no failed jobs on this run (or all jobs are still in progress)",
                "sections": [],
            })
            .to_string());
        }
        for (jid, name) in jobs {
            let log = match fetch_job_log(&client, &token, owner, repo, jid) {
                Ok(s) => s,
                Err(e) => {
                    sections.push(json!({
                        "job_id": jid,
                        "job_name": name,
                        "error": e,
                    }));
                    continue;
                }
            };
            sections.push(json!({
                "job_id": jid,
                "job_name": name,
                "lines": extract_error_lines(&log),
            }));
        }
    }

    Ok(json!({
        "owner": owner,
        "repo": repo,
        "pr": pr,
        "run_id": explicit_run,
        "job_id": explicit_job,
        "sections": sections,
    })
    .to_string())
}

fn fetch_job_log(
    client: &reqwest::blocking::Client,
    token: &str,
    owner: &str,
    repo: &str,
    job_id: i64,
) -> Result<String, String> {
    // GitHub redirects this endpoint to a signed URL — reqwest follows
    // redirects by default. Response body is plain text.
    let url = format!("https://api.github.com/repos/{owner}/{repo}/actions/jobs/{job_id}/logs");
    let resp = github_get(client, &url, token)
        .send()
        .map_err(|e| format!("gh_workflow_logs: fetch job {job_id}: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!(
            "gh_workflow_logs: job {job_id} log not available (already expired?)"
        ));
    }
    if !resp.status().is_success() {
        return Err(format!(
            "gh_workflow_logs: job {job_id} HTTP {}",
            resp.status()
        ));
    }
    resp.text()
        .map_err(|e| format!("gh_workflow_logs: job {job_id} read body: {e}"))
}

fn resolve_run_from_pr(
    client: &reqwest::blocking::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_num: i64,
) -> Result<i64, String> {
    let pr_url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{pr_num}");
    let pr_resp = github_get(client, &pr_url, token)
        .send()
        .map_err(|e| format!("gh_workflow_logs: PR lookup failed: {e}"))?;
    if !pr_resp.status().is_success() {
        return Err(format!(
            "gh_workflow_logs: PR {owner}/{repo}#{pr_num} HTTP {}",
            pr_resp.status()
        ));
    }
    let pr: Value = pr_resp
        .json()
        .map_err(|e| format!("gh_workflow_logs: PR parse failed: {e}"))?;
    let head_sha = pr
        .pointer("/head/sha")
        .and_then(Value::as_str)
        .ok_or("gh_workflow_logs: PR has no head sha")?;

    let runs_url = format!("https://api.github.com/repos/{owner}/{repo}/actions/runs");
    let runs_resp = github_get(client, &runs_url, token)
        .query(&[("head_sha", head_sha), ("per_page", "20")])
        .send()
        .map_err(|e| format!("gh_workflow_logs: runs request failed: {e}"))?;
    if !runs_resp.status().is_success() {
        return Err(format!(
            "gh_workflow_logs: runs HTTP {}",
            runs_resp.status()
        ));
    }
    let data: Value = runs_resp
        .json()
        .map_err(|e| format!("gh_workflow_logs: runs parse failed: {e}"))?;
    let runs = data
        .get("workflow_runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for run in &runs {
        let status = run.get("status").and_then(Value::as_str).unwrap_or("");
        let conclusion = run.get("conclusion").and_then(Value::as_str).unwrap_or("");
        if status == "completed"
            && matches!(
                conclusion,
                "failure" | "timed_out" | "cancelled" | "action_required"
            )
        {
            if let Some(id) = run.get("id").and_then(Value::as_i64) {
                return Ok(id);
            }
        }
    }
    Err(format!(
        "gh_workflow_logs: no failed workflow run found for {owner}/{repo}#{pr_num} (head {head_sha})"
    ))
}

fn list_failed_jobs(
    client: &reqwest::blocking::Client,
    token: &str,
    owner: &str,
    repo: &str,
    run_id: i64,
) -> Result<Vec<(i64, String)>, String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/actions/runs/{run_id}/jobs");
    let resp = github_get(client, &url, token)
        .query(&[("per_page", "30")])
        .send()
        .map_err(|e| format!("gh_workflow_logs: list jobs failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("gh_workflow_logs: jobs HTTP {}", resp.status()));
    }
    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_workflow_logs: jobs parse failed: {e}"))?;
    let jobs = data
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<(i64, String)> = Vec::new();
    for job in &jobs {
        let conclusion = job.get("conclusion").and_then(Value::as_str).unwrap_or("");
        if matches!(
            conclusion,
            "failure" | "timed_out" | "cancelled" | "action_required"
        ) {
            if let Some(id) = job.get("id").and_then(Value::as_i64) {
                let name = job
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                out.push((id, name));
            }
        }
    }
    Ok(out)
}

/// Pull error-relevant lines out of a workflow log. We capture lines
/// containing common failure markers plus a small window of context
/// around each. Caps at 80 retained lines and 8 KB total so the brain
/// doesn't get drowned in a 50 MB build log.
fn extract_error_lines(log: &str) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "error:",
        "Error:",
        "FAILED",
        "panicked at",
        "##[error]",
        "fatal:",
        "Process completed with exit code",
    ];
    const CONTEXT: usize = 2;
    const MAX_LINES: usize = 80;
    const MAX_BYTES: usize = 8 * 1024;

    let lines: Vec<&str> = log.lines().collect();
    let mut keep: Vec<bool> = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if MARKERS.iter().any(|m| line.contains(m)) {
            let start = i.saturating_sub(CONTEXT);
            let end = (i + CONTEXT + 1).min(lines.len());
            for k in keep.iter_mut().take(end).skip(start) {
                *k = true;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if out.len() >= MAX_LINES || bytes + line.len() > MAX_BYTES {
            out.push("... [truncated]".to_string());
            break;
        }
        bytes += line.len();
        out.push((*line).to_string());
    }
    out
}

fn run_gh_fork(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_fork")?;
    let owner = extract_str(&v, "owner", "gh_fork")?;
    let repo = extract_str(&v, "repo", "gh_fork")?;

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/forks");

    let resp = github_post(&client, &url, &token)
        .json(&json!({}))
        .send()
        .map_err(|e| format!("gh_fork: request failed: {e}"))?;

    let status = resp.status();
    // 202 Accepted is the documented success — fork creation is async.
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "gh_fork: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_fork: parse failed: {e}"))?;

    Ok(json!({
        "ok": true,
        "full_name": data.get("full_name").and_then(Value::as_str).unwrap_or(""),
        "clone_url": data.get("clone_url").and_then(Value::as_str).unwrap_or(""),
        "html_url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
        "default_branch": data.get("default_branch").and_then(Value::as_str).unwrap_or(""),
        "note": "fork creation is asynchronous — give it a few seconds before cloning",
    })
    .to_string())
}

fn run_gh_create_pr(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "gh_create_pr")?;
    let owner = extract_str(&v, "owner", "gh_create_pr")?;
    let repo = extract_str(&v, "repo", "gh_create_pr")?;
    let title = extract_str(&v, "title", "gh_create_pr")?;
    let head = extract_str(&v, "head", "gh_create_pr")?;
    let base = extract_str(&v, "base", "gh_create_pr")?;
    let body = v.get("body").and_then(Value::as_str).unwrap_or("");
    let draft = v.get("draft").and_then(Value::as_bool).unwrap_or(false);

    let token = github_token()?;
    let client = external_http_client()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls");
    let payload = json!({
        "title": title,
        "body": body,
        "head": head,
        "base": base,
        "draft": draft,
    });

    let resp = github_post(&client, &url, &token)
        .json(&payload)
        .send()
        .map_err(|e| format!("gh_create_pr: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "gh_create_pr: HTTP {status}: {}",
            text.chars().take(400).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("gh_create_pr: parse failed: {e}"))?;

    Ok(json!({
        "ok": true,
        "number": data.get("number").and_then(Value::as_i64).unwrap_or(0),
        "url": data.get("html_url").and_then(Value::as_str).unwrap_or(""),
        "head": head,
        "base": base,
        "draft": draft,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_get_issue_rejects_missing_fields() {
        let err = run_gh_get_issue(r#"{"owner":"example-org"}"#).unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn gh_create_issue_rejects_missing_title() {
        let err = run_gh_create_issue(r#"{"owner":"me","repo":"r"}"#).unwrap_err();
        assert!(err.contains("title"), "got: {err}");
    }

    #[test]
    fn gh_comment_issue_rejects_missing_body() {
        let err = run_gh_comment_issue(r#"{"owner":"me","repo":"r","number":1}"#).unwrap_err();
        assert!(err.contains("body"), "got: {err}");
    }

    #[test]
    fn gh_search_code_rejects_missing_query() {
        let err = run_gh_search_code("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn github_token_error_mentions_env_and_file() {
        // If the user has GITHUB_TOKEN set in their real env the result is
        // Ok and we skip the assertion — otherwise the error must guide the
        // user to both the env var and the secrets-file path.
        if let Err(msg) = github_token() {
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
    fn schemas_lists_eleven_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 11);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "gh_inbox",
                "gh_get_issue",
                "gh_create_issue",
                "gh_comment_issue",
                "gh_search_code",
                "gh_list_repo_issues",
                "gh_pr_status",
                "gh_pr_view",
                "gh_workflow_logs",
                "gh_fork",
                "gh_create_pr",
            ]
        );
    }

    #[test]
    fn gh_workflow_logs_requires_resolver_arg() {
        let err = run_gh_workflow_logs(r#"{"owner":"me","repo":"r"}"#).unwrap_err();
        assert!(
            err.contains("'pr'") || err.contains("'run_id'") || err.contains("'job_id'"),
            "error must enumerate the resolver args: {err}"
        );
    }

    #[test]
    fn gh_workflow_logs_rejects_missing_owner() {
        let err = run_gh_workflow_logs(r#"{"repo":"r","pr":1}"#).unwrap_err();
        assert!(err.contains("owner"), "got: {err}");
    }

    #[test]
    fn extract_error_lines_keeps_markers_and_context() {
        let log = "step 1\n\
                   step 2\n\
                   error: build failed\n\
                   step 4\n\
                   step 5\n\
                   step 6\n\
                   step 7\n\
                   FAILED test_x\n\
                   step 9\n";
        let kept = super::extract_error_lines(log);
        // 'error: build failed' brings context lines 1..=4 (0-indexed 0..=4
        // because i=2 with CONTEXT=2). FAILED at i=7 brings 5..=9 (clamped).
        // Both ranges merge; the result must include both markers + their
        // ± 2 line context.
        let joined = kept.join("\n");
        assert!(joined.contains("error: build failed"), "got: {joined}");
        assert!(joined.contains("FAILED test_x"), "got: {joined}");
        assert!(joined.contains("step 2"), "context not preserved: {joined}");
    }

    #[test]
    fn extract_error_lines_truncates_huge_logs() {
        use std::fmt::Write as _;
        let mut log = String::new();
        for i in 0..1000 {
            let _ = writeln!(log, "error: line {i}");
        }
        let kept = super::extract_error_lines(&log);
        assert!(
            kept.len() <= 81,
            "should cap retained lines, got {}",
            kept.len()
        );
        assert!(
            kept.iter().any(|l| l.contains("truncated")),
            "missing truncation marker"
        );
    }

    #[test]
    fn gh_pr_view_rejects_missing_number() {
        let err = run_gh_pr_view(r#"{"owner":"me","repo":"r"}"#).unwrap_err();
        assert!(err.contains("number"), "got: {err}");
    }

    #[test]
    fn gh_pr_view_rejects_missing_owner() {
        let err = run_gh_pr_view(r#"{"repo":"r","number":1}"#).unwrap_err();
        assert!(err.contains("owner"), "got: {err}");
    }

    #[test]
    fn gh_inbox_rejects_missing_scope() {
        let err = run_gh_inbox("{}").unwrap_err();
        assert!(err.contains("scope"), "got: {err}");
    }

    #[test]
    fn gh_inbox_rejects_unknown_scope() {
        let err = run_gh_inbox(r#"{"scope":"banana"}"#).unwrap_err();
        assert!(err.contains("unknown scope"), "got: {err}");
        assert!(
            err.contains("my_prs") && err.contains("assigned") && err.contains("repo_issues"),
            "error must enumerate valid scopes: {err}"
        );
    }

    #[test]
    fn gh_inbox_repo_issues_forwards_owner_repo_validation() {
        // scope='repo_issues' without owner/repo must surface the missing-
        // field error from run_gh_list_repo_issues, not a generic scope error.
        let err = run_gh_inbox(r#"{"scope":"repo_issues"}"#).unwrap_err();
        assert!(err.contains("owner") || err.contains("repo"), "got: {err}");
    }

    #[test]
    fn gh_list_repo_issues_rejects_missing_repo() {
        let err = run_gh_list_repo_issues(r#"{"owner":"me"}"#).unwrap_err();
        assert!(err.contains("repo"), "got: {err}");
    }

    #[test]
    fn gh_pr_status_rejects_missing_number() {
        let err = run_gh_pr_status(r#"{"owner":"me","repo":"r"}"#).unwrap_err();
        assert!(err.contains("number"), "got: {err}");
    }

    #[test]
    fn gh_fork_rejects_missing_repo() {
        let err = run_gh_fork(r#"{"owner":"me"}"#).unwrap_err();
        assert!(err.contains("repo"), "got: {err}");
    }

    #[test]
    fn gh_create_pr_rejects_missing_head() {
        let err =
            run_gh_create_pr(r#"{"owner":"me","repo":"r","title":"t","base":"main"}"#).unwrap_err();
        assert!(err.contains("head"), "got: {err}");
    }

    #[test]
    fn gh_create_pr_rejects_missing_base() {
        let err = run_gh_create_pr(r#"{"owner":"me","repo":"r","title":"t","head":"feature"}"#)
            .unwrap_err();
        assert!(err.contains("base"), "got: {err}");
    }
}
