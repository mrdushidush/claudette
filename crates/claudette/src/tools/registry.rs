//! Registry group — package-registry lookups against crates.io and npmjs.org.
//!
//! Two tools, both stateless HTTP. Advertised to the model on demand via the
//! `registry` tool group (see [`crate::tool_groups::ToolGroup::Registry`]).
//!
//! Sprint v0.6.0 (2026-05-21) decom dropped `crate_search` and `npm_search`
//! — both had zero positive invocations in the 100-prompt sweep because
//! `web_search` reaches the same listings with better recall and an
//! already-loaded schema. The two `_info` tools stay because they hit the
//! structured registry APIs and return canonical version + download numbers
//! that scraping wouldn't.
//!
//! Self-contained: the only parent-module helpers used are the generic
//! `parse_json_input`, `extract_str`, and `external_http_client` re-exports.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

/// Schema definitions for the two registry tools, in the same shape the model
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
    ]
}

/// Try to dispatch a tool name to one of this group's handlers. Returns
/// `None` when `name` is not a registry tool; `Some(result)` when the
/// group handled the call (successfully or with a tool-level error).
pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "crate_info" => run_crate_info(input),
        "npm_info" => run_npm_info(input),
        _ => return None,
    };
    Some(result)
}

fn run_crate_info(input: &str) -> Result<String, String> {
    // Air-gap guard: refuse before opening a socket under --offline (crates.io
    // is not on the allow-list). At the top so the refusal precedes even input
    // parsing — mirrors the facts.rs pattern.
    crate::egress::guard("https://crates.io")?;

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

fn run_npm_info(input: &str) -> Result<String, String> {
    // Air-gap guard: refuse before opening a socket under --offline (the npm
    // registry is not on the allow-list). At the top so the refusal precedes
    // even input parsing — mirrors the facts.rs pattern.
    crate::egress::guard("https://registry.npmjs.org")?;

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn schemas_lists_two_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 2);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["crate_info", "npm_info"]);
    }
}
