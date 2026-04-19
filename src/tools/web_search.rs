//! Web-search group — 1 tool (web_search) against the Brave Search API.
//! API key comes from `crate::secrets::read_secret("brave")` with a
//! BRAVE_API_KEY env-var fallback (the original env-var name pre-dates
//! the unified secret store and is kept for backwards compatibility).
//!
//! Self-contained: no private helpers. Uses the pub(super) parent helper
//! `external_http_client` and `crate::secrets` directly.

use serde_json::{json, Value};

use super::external_http_client;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
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
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "web_search" => run_web_search(input),
        _ => return None,
    };
    Some(result)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_rejects_missing_query() {
        let err = run_web_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
        assert!(err.contains("query"), "got: {err}");
    }

    #[test]
    fn web_search_rejects_invalid_json() {
        let err = run_web_search("not json").unwrap_err();
        assert!(err.contains("invalid JSON"), "got: {err}");
    }

    #[test]
    fn schemas_lists_one_tool() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 1);
        let name = schemas[0]
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(name, "web_search");
    }
}
