//! Action transcript + trash — recoverability for destructive operations.
//!
//! Destruction used to be irreversible and invisible: `note_delete` /
//! `todo_delete` removed data permanently and `write_file` silently
//! truncated existing files. A weak local model that misroutes "clean up my
//! notes" destroyed real user data with nothing to recover (a roast flagged
//! exactly this bulk-delete gap). For an autonomous agent acting on a
//! user's files, *recoverability is itself a feature*. This module gives
//! every destructive op a pre-image and every mutating tool call a log line:
//!
//! - **Trash** (`~/.claudette/trash/`): [`move_to_trash`] relocates a file
//!   instead of deleting it; [`snapshot_to_trash`] copies a file about to be
//!   overwritten. Timestamp-prefixed names prevent collisions.
//! - **Transcript** (`~/.claudette/transcript/actions.jsonl`): [`record`]
//!   appends one JSON line per **mutating** tool call (`ReadOnly` tools are
//!   never logged — that would be noise and a privacy footgun). Best-effort:
//!   a failed transcript write never fails the tool call.
//! - **`/undo`**: [`undo_last`] restores the most recent not-yet-undone
//!   entry that carries an undo ref, then appends an `undo` entry so the
//!   log stays truthful. One step at a time, no stack.
//!
//! Everything stays under `~/.claudette/` — local-only, never uploaded,
//! consistent with the privacy posture in `PRIVACY.md`.

use std::cell::RefCell;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Cap on the `input` field stored per transcript line. The undo ref carries
/// the real pre-image; the input is for audit readability, and an
/// uncapped `write_file` content would balloon the log.
const MAX_RECORDED_INPUT_CHARS: usize = 2_000;

fn home_dir() -> PathBuf {
    let raw = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(raw)
}

pub(crate) fn trash_dir() -> PathBuf {
    home_dir().join(".claudette").join("trash")
}

pub(crate) fn transcript_path() -> PathBuf {
    home_dir()
        .join(".claudette")
        .join("transcript")
        .join("actions.jsonl")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Pick a collision-free target path in the trash for `original`'s filename:
/// `<unix_ms>-<filename>`, with a `-1`, `-2`, … suffix if two ops land on
/// the same millisecond + name.
fn trash_target_for(original: &Path) -> std::io::Result<PathBuf> {
    let dir = trash_dir();
    fs::create_dir_all(&dir)?;
    let filename = original.file_name().map_or_else(
        || "unnamed".to_string(),
        |f| f.to_string_lossy().to_string(),
    );
    let base = format!("{}-{filename}", now_ms());
    let mut candidate = dir.join(&base);
    let mut n = 0u32;
    while candidate.exists() {
        n += 1;
        candidate = dir.join(format!("{base}-{n}"));
    }
    Ok(candidate)
}

thread_local! {
    /// The undo ref produced by the most recent trash operation on this
    /// thread. Tools run synchronously on the worker thread, so the
    /// executor can [`take_pending_undo`] right after dispatch — same
    /// pattern as `tools::set_current_turn_paths`.
    static PENDING_UNDO: RefCell<Option<Value>> = const { RefCell::new(None) };
}

fn set_pending_undo(trash: &Path, original: &Path) {
    let v = json!({
        "trash": trash.display().to_string(),
        "original": original.display().to_string(),
    });
    PENDING_UNDO.with(|p| *p.borrow_mut() = Some(v));
}

/// Take (and clear) the undo ref left behind by the last trash op on this
/// thread. Called by the executor after a successful tool dispatch.
#[must_use]
pub fn take_pending_undo() -> Option<Value> {
    PENDING_UNDO.with(|p| p.borrow_mut().take())
}

/// Move `original` into the trash instead of deleting it. Returns the trash
/// path. Falls back to copy+remove when `rename` crosses filesystems.
/// Leaves an undo ref for [`take_pending_undo`].
pub fn move_to_trash(original: &Path) -> std::io::Result<PathBuf> {
    let target = trash_target_for(original)?;
    if fs::rename(original, &target).is_err() {
        // Cross-device (or exotic fs) — copy then remove.
        fs::copy(original, &target)?;
        fs::remove_file(original)?;
    }
    set_pending_undo(&target, original);
    Ok(target)
}

/// Copy `original` into the trash, leaving the original in place — the
/// pre-image for a file that is about to be overwritten. Leaves an undo
/// ref for [`take_pending_undo`].
pub fn snapshot_to_trash(original: &Path) -> std::io::Result<PathBuf> {
    let target = trash_target_for(original)?;
    fs::copy(original, &target)?;
    set_pending_undo(&target, original);
    Ok(target)
}

/// Append one transcript line for a mutating tool call. **Best-effort**: a
/// failed write logs to stderr and returns — it must never fail the tool
/// call it describes.
pub fn record(tool: &str, input: &str, undo: Option<Value>) {
    let capped: String = if input.chars().count() > MAX_RECORDED_INPUT_CHARS {
        let head: String = input.chars().take(MAX_RECORDED_INPUT_CHARS).collect();
        format!("{head}… [capped, {} chars total]", input.chars().count())
    } else {
        input.to_string()
    };
    let line = json!({
        "ts": now_ms() as u64,
        "tool": tool,
        "input": capped,
        "undo": undo.unwrap_or(Value::Null),
    });
    let path = transcript_path();
    let write = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{line}")
    })();
    if let Err(e) = write {
        eprintln!("transcript: record failed (tool call unaffected): {e}");
    }
}

/// Undo the most recent transcript entry that (a) carries an undo ref and
/// (b) hasn't already been undone. Restores the trashed/pre-image file back
/// to its original location (the trash copy is **kept** — recoverability
/// bias), appends an `undo` entry, and returns a human-readable summary.
pub fn undo_last() -> Result<String, String> {
    let path = transcript_path();
    let raw = fs::read_to_string(&path)
        .map_err(|_| "nothing to undo (no actions recorded yet)".to_string())?;

    let entries: Vec<Value> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    // Entries already reverted by a previous /undo — keyed on the TRASH
    // PATH, which is collision-free by construction (the `-1`/`-2` suffix
    // loop in `trash_target_for`). Keying on `ts` broke batch deletes: a
    // model emitting 3 note_deletes in one assistant message runs them in
    // a sub-millisecond loop, all sharing one ms timestamp — undoing the
    // first marked the SIBLINGS as undone too, making them unreachable
    // (caught by the adversarial review with a standalone repro).
    let undone: Vec<&str> = entries
        .iter()
        .filter(|e| e.get("tool").and_then(Value::as_str) == Some("undo"))
        .filter_map(|e| e.get("undone_trash").and_then(Value::as_str))
        .collect();

    let target = entries.iter().rev().find(|e| {
        e.get("undo")
            .and_then(|u| u.get("trash"))
            .and_then(Value::as_str)
            .is_some_and(|t| !undone.contains(&t))
    });
    let Some(entry) = target else {
        return Err(
            "nothing to undo (no recorded action carries a recoverable pre-image)".to_string(),
        );
    };

    let ts = entry.get("ts").and_then(Value::as_u64).unwrap_or(0);
    let tool = entry
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let undo_ref = entry.get("undo").cloned().unwrap_or(Value::Null);
    let trash = undo_ref
        .get("trash")
        .and_then(Value::as_str)
        .ok_or("transcript entry has a malformed undo ref (no trash path)")?;
    let original = undo_ref
        .get("original")
        .and_then(Value::as_str)
        .ok_or("transcript entry has a malformed undo ref (no original path)")?;

    let trash_p = Path::new(trash);
    if !trash_p.exists() {
        return Err(format!(
            "cannot undo {tool}: the trash copy is gone ({trash})"
        ));
    }
    let original_p = Path::new(original);
    if let Some(parent) = original_p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot undo: {e}"))?;
    }
    // Copy (not move): the trash copy stays as belt-and-braces until the
    // user empties the trash themselves.
    fs::copy(trash_p, original_p)
        .map_err(|e| format!("cannot undo {tool}: restore to {original} failed ({e})"))?;

    // Log the undo itself so a second /undo moves on to the previous
    // action. `undone_trash` is the dedup key (collision-free);
    // `undone_ts` stays for human audit. A failed marker write is loud:
    // /undo would otherwise silently re-offer the same entry next time.
    let line = json!({
        "ts": now_ms() as u64,
        "tool": "undo",
        "input": "",
        "undo": Value::Null,
        "undone_ts": ts,
        "undone_trash": trash,
    });
    let marker = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = marker {
        eprintln!(
            "transcript: undo marker write failed ({e}) — a repeat /undo will \
             re-restore the same entry"
        );
    }

    Ok(format!(
        "restored {original} from trash (undid {tool}; the trash copy at {trash} is kept)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::with_temp_home;

    fn write_tmp(home: &Path, name: &str, content: &str) -> PathBuf {
        let p = home.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn move_to_trash_relocates_and_survives_name_collision() {
        with_temp_home(|home| {
            let a = write_tmp(home, "victim.md", "first");
            let t1 = move_to_trash(&a).unwrap();
            assert!(!a.exists(), "original should be gone");
            assert_eq!(fs::read_to_string(&t1).unwrap(), "first");

            // Same filename again — must land on a DIFFERENT trash path.
            let b = write_tmp(home, "victim.md", "second");
            let t2 = move_to_trash(&b).unwrap();
            assert_ne!(t1, t2, "collision must produce distinct trash names");
            assert_eq!(fs::read_to_string(&t2).unwrap(), "second");
            assert_eq!(fs::read_to_string(&t1).unwrap(), "first");
        });
    }

    #[test]
    fn snapshot_keeps_the_original_in_place() {
        with_temp_home(|home| {
            let a = write_tmp(home, "live.txt", "pre-image");
            let t = snapshot_to_trash(&a).unwrap();
            assert!(a.exists(), "snapshot must not remove the original");
            assert_eq!(fs::read_to_string(&t).unwrap(), "pre-image");
        });
    }

    #[test]
    fn record_and_undo_round_trip() {
        with_temp_home(|home| {
            let a = write_tmp(home, "note.md", "precious");
            let _t = move_to_trash(&a).unwrap();
            record("note_delete", r#"{"id":"note.md"}"#, take_pending_undo());
            assert!(!a.exists());

            let msg = undo_last().expect("undo should succeed");
            assert!(a.exists(), "undo must restore the file: {msg}");
            assert_eq!(fs::read_to_string(&a).unwrap(), "precious");

            // The undo itself was logged → a second undo finds nothing left.
            let err = undo_last().unwrap_err();
            assert!(err.contains("nothing to undo"), "got: {err}");
        });
    }

    #[test]
    fn undo_twice_restores_both_entries_of_a_same_millisecond_batch() {
        // Regression (adversarial review): a model emitting multiple deletes
        // in ONE assistant message runs them in a sub-ms loop, so their
        // transcript `ts` values collide. The undone-set must key on the
        // collision-free trash path, not ts — otherwise undoing the first
        // marks the siblings as undone and /undo reports "nothing to undo".
        with_temp_home(|home| {
            let a = write_tmp(home, "a.md", "AAA");
            let b = write_tmp(home, "b.md", "BBB");
            // Tight loop — same millisecond in practice, and the assertion
            // below must hold regardless.
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());
            let _ = move_to_trash(&b).unwrap();
            record("note_delete", "b", take_pending_undo());

            let msg1 = undo_last().expect("undo #1");
            assert!(msg1.contains("b.md"), "most recent first: {msg1}");
            assert!(b.exists());

            let msg2 = undo_last().expect("undo #2 — same-ms sibling must remain reachable");
            assert!(msg2.contains("a.md"), "got: {msg2}");
            assert!(a.exists());

            let err = undo_last().unwrap_err();
            assert!(err.contains("nothing to undo"), "got: {err}");
        });
    }

    #[test]
    fn undo_with_no_transcript_says_so() {
        with_temp_home(|_| {
            let err = undo_last().unwrap_err();
            assert!(err.contains("nothing to undo"), "got: {err}");
        });
    }

    #[test]
    fn undo_skips_entries_without_refs_and_walks_backwards() {
        with_temp_home(|home| {
            let a = write_tmp(home, "a.md", "AAA");
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());
            // A mutating-but-not-undoable entry on top (e.g. todo_add).
            record("todo_add", "whatever", None);

            let msg = undo_last().expect("should undo the delete underneath");
            assert!(msg.contains("a.md"), "got: {msg}");
            assert!(a.exists());
        });
    }

    #[test]
    fn record_caps_oversized_input() {
        with_temp_home(|_| {
            let huge = "x".repeat(10_000);
            record("write_file", &huge, None);
            let raw = fs::read_to_string(transcript_path()).unwrap();
            let v: Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
            let stored = v.get("input").and_then(Value::as_str).unwrap();
            assert!(stored.chars().count() < 2_100, "input must be capped");
            assert!(stored.contains("capped"), "cap marker missing");
        });
    }

    #[test]
    fn pending_undo_is_take_once() {
        with_temp_home(|home| {
            let a = write_tmp(home, "x.txt", "1");
            let _ = move_to_trash(&a).unwrap();
            assert!(take_pending_undo().is_some());
            assert!(
                take_pending_undo().is_none(),
                "second take must be empty — no stale undo may leak to the next tool call"
            );
        });
    }
}
