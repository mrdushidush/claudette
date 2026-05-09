//! Registry group — package-registry lookups against crates.io and npmjs.org.
//!
//! Four tools, all stateless HTTP. Advertised to the model on demand via the
//! `registry` tool group (see [`crate::tool_groups::ToolGroup::Registry`]).
//!
//! Self-contained: the only parent-module helpers used are the generic
//! `parse_json_input`, `extract_str`, and `external_http_client` re-exports.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

/// Schema definitions for the four registry tools, in the same shape the model
/// sees in the live Ollama request.
pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "crate_info",
                "description": "Get metadata for a Rust crate on crates.io: latest version, description, downloads, homepage.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Crate name (e.g. 'tokio')" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "crate_search",
                "description": "Search crates.io for Rust crates. Returns top 5 by downloads.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms" }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "npm_info",
                "description": "Get metadata for an npm package: latest version, description, homepage, weekly downloads.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Package name (e.g. 'react' or '@scope/pkg')" }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "npm_search",
                "description": "Search npmjs.org for packages. Returns top 5 hits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms" }
                    },
                    "required": ["query"]
                }
            }
        }),
    ]
}

/// Try to dispatch a tool name to one of this group's handlers. Returns
/// `None` when `name` is not a registry tool; `Some(result)` when the
/// group handled the call (successfully or with a tool-level error).
pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "crate_info" => run_crate_info(input),
        "crate_search" => run_crate_search(input),
        "npm_info" => run_npm_info(input),
        "npm_search" => run_npm_search(input),
        _ => return None,
    };
    Some(result)
}

fn run_crate_info(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "crate_info")?;
    let name = extract_str(&v, "name", "crate_info")?;
    let url = format!("https://crates.io/api/v1/crates/{name}");

    let client = external_http_client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("crate_info: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("crate_info: no crate named '{name}'"));
    }
    if !status.is_success() {
        return Err(format!("crate_info: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("crate_info: parse failed: {e}"))?;

    let krate = data
        .get("crate")
        .ok_or("crate_info: response missing 'crate'")?;

    Ok(json!({
        "name": krate.get("name").and_then(Value::as_str).unwrap_or(name),
        "description": krate.get("description").and_then(Value::as_str).unwrap_or(""),
        "latest_version": krate.get("max_stable_version").and_then(Value::as_str)
            .or_else(|| krate.get("max_version").and_then(Value::as_str))
            .unwrap_or(""),
        "downloads": krate.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        "recent_downloads": krate.get("recent_downloads").and_then(Value::as_u64).unwrap_or(0),
        "homepage": krate.get("homepage").and_then(Value::as_str).unwrap_or(""),
        "repository": krate.get("repository").and_then(Value::as_str).unwrap_or(""),
        "documentation": krate.get("documentation").and_then(Value::as_str).unwrap_or(""),
        "updated_at": krate.get("updated_at").and_then(Value::as_str).unwrap_or(""),
    })
    .to_string())
}

fn run_crate_search(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "crate_search")?;
    let query = extract_str(&v, "query", "crate_search")?;

    let client = external_http_client()?;
    let resp = client
        .get("https://crates.io/api/v1/crates")
        .query(&[("q", query), ("per_page", "5"), ("sort", "downloads")])
        .send()
        .map_err(|e| format!("crate_search: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("crate_search: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("crate_search: parse failed: {e}"))?;

    let results: Vec<Value> = data
        .get("crates")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(5)
                .map(|c| {
                    json!({
                        "name": c.get("name").and_then(Value::as_str).unwrap_or(""),
                        "description": c.get("description").and_then(Value::as_str).unwrap_or(""),
                        "latest_version": c.get("max_stable_version").and_then(Value::as_str)
                            .or_else(|| c.get("max_version").and_then(Value::as_str))
                            .unwrap_or(""),
                        "downloads": c.get("downloads").and_then(Value::as_u64).unwrap_or(0),
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

fn run_npm_info(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "npm_info")?;
    let name = extract_str(&v, "name", "npm_info")?;

    let client = external_http_client()?;

    // Full package document — big, but the shape is stable.
    let url = format!("https://registry.npmjs.org/{name}");
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("npm_info: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("npm_info: no package named '{name}'"));
    }
    if !status.is_success() {
        return Err(format!("npm_info: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("npm_info: parse failed: {e}"))?;

    let latest = data
        .pointer("/dist-tags/latest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let homepage = data.get("homepage").and_then(Value::as_str).unwrap_or("");
    let repo_url = data
        .pointer("/repository/url")
        .and_then(Value::as_str)
        .unwrap_or("");
    let license = data.get("license").and_then(Value::as_str).unwrap_or("");

    // Weekly downloads via a second call — optional, best-effort.
    let downloads = client
        .get(format!(
            "https://api.npmjs.org/downloads/point/last-week/{name}"
        ))
        .send()
        .ok()
        .and_then(|r| r.json::<Value>().ok())
        .and_then(|v| v.get("downloads").and_then(Value::as_u64))
        .unwrap_or(0);

    Ok(json!({
        "name": name,
        "description": description,
        "latest_version": latest,
        "homepage": homepage,
        "repository": repo_url,
        "license": license,
        "weekly_downloads": downloads,
    })
    .to_string())
}

fn run_npm_search(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "npm_search")?;
    let query = extract_str(&v, "query", "npm_search")?;

    let client = external_http_client()?;
    let resp = client
        .get("https://registry.npmjs.org/-/v1/search")
        .query(&[("text", query), ("size", "5")])
        .send()
        .map_err(|e| format!("npm_search: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("npm_search: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("npm_search: parse failed: {e}"))?;

    let results: Vec<Value> = data
        .get("objects")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .take(5)
                .map(|o| {
                    let pkg = o.get("package").unwrap_or(&Value::Null);
                    json!({
                        "name": pkg.get("name").and_then(Value::as_str).unwrap_or(""),
                        "description": pkg.get("description").and_then(Value::as_str).unwrap_or(""),
                        "latest_version": pkg.get("version").and_then(Value::as_str).unwrap_or(""),
                        "homepage": pkg.pointer("/links/homepage").and_then(Value::as_str).unwrap_or(""),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Sprint 9 Phase 0a — input validation for new tools. No network.

    #[test]
    fn crate_info_rejects_missing_name() {
        let err = run_crate_info("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn npm_info_rejects_missing_name() {
        let err = run_npm_info("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    // Bonus: confirm the schema list stays pinned at four entries
    // so additions aren't silently dropped.
    #[test]
    fn schemas_lists_four_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 4);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            ["crate_info", "crate_search", "npm_info", "npm_search"]
        );
    }
}
