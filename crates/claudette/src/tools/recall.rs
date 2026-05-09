//! Recall group — the `recall(query, k)` tool.
//!
//! Thin adapter over [`crate::recall`]: parses the JSON args, calls
//! [`crate::recall::global_query`], and serialises the hits back as a
//! compact JSON object the brain can summarise. The actual embedding
//! and SQLite work lives in `src/recall.rs`.

use serde_json::{json, Value};

use super::parse_json_input;
use crate::recall;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "recall",
            "description": "Search the cross-session memory for relevant past messages. Returns the top-k by semantic similarity. Use when the user references something from a past conversation ('what did I tell you about X', 'remember when we discussed Y').",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural-language search query." },
                    "k":     { "type": "number", "description": "Max hits (default 5, max 20)." }
                },
                "required": ["query"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    if name != "recall" {
        return None;
    }
    Some(run_recall(input))
}

fn run_recall(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "recall")?;
    let query = v
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| "recall: missing or non-string 'query'".to_string())?;
    // k: clamp to [1, 20]. Default 5 (per spec). Brain can ask for more
    // when scanning a topic; cap protects context.
    let k_raw = v.get("k").and_then(Value::as_u64).unwrap_or(5);
    let k = k_raw.clamp(1, 20) as usize;

    let hits = recall::global_query(query, k)?;
    let results: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "ts": h.ts,
                "role": match h.role {
                    recall::Role::User => "user",
                    recall::Role::Assistant => "assistant",
                },
                "snippet": h.snippet,
                "score": format!("{:.3}", h.score),
            })
        })
        .collect();
    Ok(json!({
        "query": query,
        "k": k,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_one_tool() {
        let s = schemas();
        assert_eq!(s.len(), 1);
        let name = s[0]
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(name, "recall");
    }

    #[test]
    fn dispatch_returns_none_for_other_names() {
        assert!(dispatch("note_create", "{}").is_none());
        assert!(dispatch("recall_other", "{}").is_none());
    }

    #[test]
    fn run_recall_rejects_missing_query() {
        let err = run_recall("{}").unwrap_err();
        assert!(err.contains("query"), "got: {err}");
    }

    #[test]
    fn run_recall_rejects_non_json() {
        let err = run_recall("not json").unwrap_err();
        assert!(err.contains("invalid JSON"), "got: {err}");
    }
}
