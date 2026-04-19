//! GitHub group — 6 tools against the REST API. Token comes from
//! `crate::secrets::read_secret("github")` (GITHUB_TOKEN env or
//! `~/.claudette/secrets/github.token`).
//!
//! Self-contained: all helpers (`github_token`, `github_get`, `github_post`,
//! `github_me`, `github_search_issues`) are private to this module.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "gh_list_my_prs",
                "description": "List open pull requests I authored. Requires GITHUB_TOKEN in env.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "gh_list_assigned_issues",
                "description": "List open issues assigned to me. Requires GITHUB_TOKEN in env.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
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
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "gh_list_my_prs" => run_gh_list_my_prs(),
        "gh_list_assigned_issues" => run_gh_list_assigned_issues(),
        "gh_get_issue" => run_gh_get_issue(input),
        "gh_create_issue" => run_gh_create_issue(input),
        "gh_comment_issue" => run_gh_comment_issue(input),
        "gh_search_code" => run_gh_search_code(input),
        _ => return None,
    };
    Some(result)
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
    fn schemas_lists_six_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 6);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "gh_list_my_prs",
                "gh_list_assigned_issues",
                "gh_get_issue",
                "gh_create_issue",
                "gh_comment_issue",
                "gh_search_code",
            ]
        );
    }
}
