//! Forge-tail group — `forge_tail`. Sprint v0.6.0 Phase 3.4c. Closes the
//! "forge mission is a 100-second blackbox" gap surfaced in the
//! 2026-05-16 round-3 sweep memo by giving the brain a way to peek at
//! the latest Planner / Coder / Verifier output mid-run.
//!
//! Storage convention: forge worker writes its stderr to
//! `~/.claudette/forge/<mission_id>.log`. If the worker hasn't been
//! taught to write that log yet (the live capture is a follow-up to
//! this sprint), forge_tail returns a clear "no log found" message
//! with the looked-up path so the user can wire the missing producer.
//!
//! Active-mission resolution: if `mission_id` is omitted we pick the
//! currently-active mission from [`crate::missions::active_mission`].

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{claudette_home, parse_json_input};

const DEFAULT_LINES: usize = 50;
const MAX_LINES: usize = 500;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "forge_tail",
            "description": "Tail the latest Planner/Coder/Verifier output for a forge mission. Defaults to the active mission. Returns the last `lines` (default 50) from ~/.claudette/forge/<id>.log.",
            "parameters": {
                "type": "object",
                "properties": {
                    "mission_id": { "type": "string", "description": "Mission slug; defaults to the active mission." },
                    "lines":      { "type": "number", "description": "Number of lines from the end (default 50, max 500)." }
                },
                "required": []
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "forge_tail" => run_forge_tail(input),
        _ => return None,
    };
    Some(result)
}

fn forge_log_dir() -> PathBuf {
    claudette_home().join("forge")
}

fn run_forge_tail(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "forge_tail")?;
    let lines = v
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LINES as u64)
        .clamp(1, MAX_LINES as u64) as usize;

    let mission_id = v
        .get("mission_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| crate::missions::active_mission().map(|m| m.slug.clone()));

    let Some(mid) = mission_id else {
        return Err(
            "forge_tail: no `mission_id` given and no active mission — start one with mission_start first, or pass mission_id explicitly."
                .to_string(),
        );
    };

    let log_path = forge_log_dir().join(format!("{mid}.log"));
    if !log_path.exists() {
        return Ok(json!({
            "mission_id": mid,
            "log_path": log_path.display().to_string(),
            "exists": false,
            "lines_returned": 0,
            "note": "no forge log on disk yet. The forge worker writes its Planner/Coder/Verifier stderr to ~/.claudette/forge/<mission_id>.log; if you don't see a file, the producer-side wiring is a follow-up to v0.6.0 — track on the sprint memo.",
            "lines": [],
        })
        .to_string());
    }

    let content = fs::read_to_string(&log_path)
        .map_err(|e| format!("forge_tail: read {} failed: {e}", log_path.display()))?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(lines);
    let tail: Vec<String> = all[start..].iter().map(|s| (*s).to_string()).collect();

    Ok(json!({
        "mission_id": mid,
        "log_path": log_path.display().to_string(),
        "exists": true,
        "total_lines": all.len(),
        "lines_returned": tail.len(),
        "lines": tail,
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
        assert_eq!(name, "forge_tail");
    }

    #[test]
    fn forge_tail_explains_when_no_active_mission_and_no_arg() {
        // Skip if there's actually an active mission left over from another
        // test — we only want to exercise the "neither" branch.
        if crate::missions::active_mission().is_some() {
            return;
        }
        let err = run_forge_tail("{}").unwrap_err();
        assert!(err.contains("no active mission"), "got: {err}");
        assert!(err.contains("mission_id"), "got: {err}");
    }

    #[test]
    fn forge_tail_reports_missing_log_for_explicit_mission_id() {
        // Use a definitely-nonexistent slug; the function should return Ok
        // with `exists: false` plus a `note` pointing at the follow-up.
        let unique = format!(
            "claudette-forge-tail-missing-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let out = run_forge_tail(&json!({ "mission_id": &unique }).to_string()).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["exists"], false);
        assert!(v["note"].as_str().unwrap_or("").contains("follow-up"));
        assert_eq!(v["lines_returned"], 0);
    }

    #[test]
    fn forge_tail_reads_existing_log() {
        // Run under an isolated HOME held by the shared env lock. forge_log_dir()
        // resolves through $HOME, and a concurrent HOME-swapping test (e.g. in
        // runtime/prompt.rs) used to yank the directory out from under us between
        // the create_dir_all and the write — flaking this test with a NotFound on
        // ubuntu CI. with_temp_home serialises against those and gives us a dir
        // nothing else touches.
        crate::with_temp_home(|_home| {
            let unique = "claudette-forge-tail-real";
            let dir = forge_log_dir();
            std::fs::create_dir_all(&dir).expect("create forge log dir");
            let log_path = dir.join(format!("{unique}.log"));
            std::fs::write(&log_path, "one\ntwo\nthree\nfour\nfive\n").unwrap();

            let out = run_forge_tail(&json!({ "mission_id": unique, "lines": 3 }).to_string())
                .expect("ok");
            let v: Value = serde_json::from_str(&out).unwrap();

            assert_eq!(v["exists"], true);
            assert_eq!(v["lines_returned"], 3);
            let lines = v["lines"].as_array().unwrap();
            assert_eq!(lines.last().unwrap(), "five");
            assert_eq!(lines.first().unwrap(), "three");
        });
    }
}
