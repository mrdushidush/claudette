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

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Cap on the `input` field stored per transcript line. The undo ref carries
/// the real pre-image; the input is for audit readability, and an
/// uncapped `write_file` content would balloon the log.
const MAX_RECORDED_INPUT_CHARS: usize = 2_000;

pub(crate) fn trash_dir() -> PathBuf {
    crate::env_config::home_dir()
        .join(".claudette")
        .join("trash")
}

pub(crate) fn transcript_path() -> PathBuf {
    crate::env_config::home_dir()
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

thread_local! {
    /// The id of the turn currently in progress, stamped onto every
    /// transcript line [`record`]ed during it so `/undo` and `/diff` can
    /// group a whole turn's actions. `None` until the first [`begin_turn`] —
    /// lines recorded outside any turn carry `turn: null` and the
    /// turn-scoped ops treat each as its own singleton (falling back to
    /// single-entry behavior).
    static CURRENT_TURN_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Per-process monotonic turn counter — the tie-breaker in the turn id.
    static TURN_COUNTER: Cell<u64> = const { Cell::new(0) };
}

/// Open a new turn: mint a fresh turn id so subsequent [`record`] lines are
/// tagged with it. Called at every turn entry point, alongside
/// `tools::set_current_turn_paths` (same thread — tools run synchronously on
/// the worker thread that recorded the turn). The id is
/// `<unix_ms>-<pid>-<counter>`, globally unique: the transcript is
/// append-only and outlives the process, so a bare counter would collide
/// with another session's turns and make "undo the last turn" ambiguous.
pub fn begin_turn() {
    let n = TURN_COUNTER.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(1));
        v
    });
    let id = format!("{}-{}-{n}", now_ms(), std::process::id());
    CURRENT_TURN_ID.with(|t| *t.borrow_mut() = Some(id));
}

fn current_turn_id() -> Option<String> {
    CURRENT_TURN_ID.with(|t| t.borrow().clone())
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
    // Mask credential-shaped substrings BEFORE capping/storing: tool input can
    // carry a PAT (e.g. a `git remote set-url` with an embedded token), and the
    // transcript is a plaintext file on disk. Redact first so the secret never
    // lands in actions.jsonl. (roast 2026-06-21, Wave 1.3)
    let redacted = crate::redact::redact(input);
    let capped: String = if redacted.chars().count() > MAX_RECORDED_INPUT_CHARS {
        let head: String = redacted.chars().take(MAX_RECORDED_INPUT_CHARS).collect();
        format!("{head}… [capped, {} chars total]", redacted.chars().count())
    } else {
        redacted.into_owned()
    };
    let line = json!({
        "ts": now_ms() as u64,
        "turn": current_turn_id(),
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

/// Read + parse the transcript into one `Value` per line (unparseable lines
/// dropped). Returns the `nothing to undo` error string when the file is
/// missing so callers can surface it verbatim.
fn read_entries() -> Result<Vec<Value>, String> {
    let raw = fs::read_to_string(transcript_path())
        .map_err(|_| "nothing to undo (no actions recorded yet)".to_string())?;
    Ok(raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Trash paths already reverted by a previous `/undo` — keyed on the TRASH
/// PATH, which is collision-free by construction (the `-1`/`-2` suffix loop
/// in `trash_target_for`). Keying on `ts` broke batch deletes: a model
/// emitting 3 note_deletes in one assistant message runs them in a
/// sub-millisecond loop, all sharing one ms timestamp — undoing the first
/// marked the SIBLINGS as undone too, making them unreachable (caught by the
/// adversarial review with a standalone repro).
fn undone_trash_set(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.get("tool").and_then(Value::as_str) == Some("undo"))
        .filter_map(|e| e.get("undone_trash").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// True when `entry` carries a recoverable pre-image not yet reverted.
fn is_recoverable(entry: &Value, undone: &[String]) -> bool {
    entry
        .get("undo")
        .and_then(|u| u.get("trash"))
        .and_then(Value::as_str)
        .is_some_and(|t| !undone.iter().any(|u| u == t))
}

/// Undo the most recent transcript entry that (a) carries an undo ref and
/// (b) hasn't already been undone — the `/undo one` behavior. Restores the
/// trashed/pre-image file back to its original location (the trash copy is
/// **kept** — recoverability bias), appends an `undo` entry, and returns a
/// human-readable summary.
pub fn undo_last() -> Result<String, String> {
    let entries = read_entries()?;
    let undone = undone_trash_set(&entries);
    let target = entries.iter().rev().find(|e| is_recoverable(e, &undone));
    let Some(entry) = target else {
        return Err(
            "nothing to undo (no recorded action carries a recoverable pre-image)".to_string(),
        );
    };
    restore_entry(entry, &transcript_path())
}

/// Undo every recoverable action of the **last turn** as one step (the
/// default `/undo`). The last turn is the turn id carried by the most recent
/// recoverable, not-yet-undone entry; a turn's entries are contiguous in the
/// append-only log, so we revert that entry and every sibling sharing its
/// turn id, **newest-first** (correct when a file was edited twice in the
/// turn — the last restore lands the turn's original pre-image). A `null`
/// turn id (recorded outside `begin_turn`) groups only with itself, degrading
/// gracefully to single-entry behavior.
pub fn undo_last_turn() -> Result<String, String> {
    let entries = read_entries()?;
    let undone = undone_trash_set(&entries);

    let Some(last) = entries.iter().rev().find(|e| is_recoverable(e, &undone)) else {
        return Err(
            "nothing to undo (no recorded action carries a recoverable pre-image)".to_string(),
        );
    };
    let turn = last.get("turn").and_then(Value::as_str).map(str::to_string);

    // Newest-first group of this turn's still-recoverable entries.
    let group: Vec<&Value> = match &turn {
        Some(id) => entries
            .iter()
            .rev()
            .filter(|e| e.get("turn").and_then(Value::as_str) == Some(id.as_str()))
            .filter(|e| is_recoverable(e, &undone))
            .collect(),
        None => vec![last],
    };

    let path = transcript_path();
    let mut summaries = Vec::new();
    let mut errors = Vec::new();
    for e in group {
        match restore_entry(e, &path) {
            Ok(s) => summaries.push(s),
            Err(err) => errors.push(err),
        }
    }
    if summaries.is_empty() {
        return Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| "nothing to undo".to_string()));
    }
    let mut out = if summaries.len() == 1 {
        summaries.remove(0)
    } else {
        let mut s = format!("undid the last turn — restored {} files:", summaries.len());
        for line in &summaries {
            s.push_str("\n  • ");
            s.push_str(line);
        }
        s
    };
    if !errors.is_empty() {
        let _ = write!(
            out,
            "\n  ({} could not be restored: {})",
            errors.len(),
            errors.join("; ")
        );
    }
    Ok(out)
}

/// Restore a single transcript `entry`'s pre-image: the shared core of
/// `undo_last` (one entry) and `undo_last_turn` (every recoverable entry of a
/// turn). Re-validates both paths against a tampered `actions.jsonl`, backs
/// up any newer content at the target first (fail-closed), copies the
/// pre-image back, and appends an `undo` marker keyed on the collision-free
/// trash path. `path` is the transcript file to append the marker to.
#[allow(clippy::too_many_lines)]
fn restore_entry(entry: &Value, path: &Path) -> Result<String, String> {
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
    // Both paths come verbatim from actions.jsonl — a plain, user-writable
    // JSON file. Re-validate them before copying (defense-in-depth, roast
    // 2026-06-07): without this a tampered line turns /undo into an
    // arbitrary file copy — any readable `trash` source to any writable
    // `original` destination (e.g. trash=~/.ssh/id_ed25519 →
    // original=<workspace>/leak.txt). Legitimate refs always have a
    // trash-dir source, and an original that some tool already validated
    // at action time — notes/todos/scratch/missions all live under HOME,
    // write_file targets live under HOME or CLAUDETTE_WORKSPACE — so the
    // READ envelope (`validate_read_path`: HOME + workspace + secret
    // denylist + symlink defence) admits every honest entry. The narrower
    // `validate_write_path` would be wrong here: it excludes
    // ~/.claudette/notes/, breaking legit note_delete undo.
    let trash_root = fs::canonicalize(trash_dir())
        .map_err(|e| format!("cannot undo {tool}: trash dir unavailable ({e})"))?;
    let trash_canon = fs::canonicalize(trash_p)
        .map_err(|e| format!("cannot undo {tool}: cannot resolve trash copy {trash} ({e})"))?;
    if !trash_canon.starts_with(&trash_root) {
        return Err(format!(
            "cannot undo {tool}: undo ref points outside the trash dir ({trash}) — \
             refusing (tampered transcript?)"
        ));
    }
    let original_owned = crate::tools::validate_read_path(original).map_err(|e| {
        format!(
            "cannot undo {tool}: restore target {original} fails path validation — \
             refusing (tampered transcript?): {e}"
        )
    })?;
    let original_p: &Path = &original_owned;
    if let Some(parent) = original_p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot undo: {e}"))?;
    }
    // Undo must never destroy data either. If the original path now holds
    // content (e.g. the file was overwritten and then edited again, or a new
    // file was created where a deleted one stood), restoring the trash copy
    // would clobber that NEWER content with no pre-image. Snapshot the
    // current content to trash FIRST — fail-closed: if we can't back it up,
    // refuse the undo rather than lose it. NOTE: a raw trash copy here, NOT
    // snapshot_to_trash(), because that sets the PENDING_UNDO thread-local —
    // /undo runs outside the executor's take_pending_undo(), so it would
    // leak onto the next tool call's transcript line.
    let mut backed_up: Option<String> = None;
    if original_p.exists() {
        let backup = trash_target_for(original_p).map_err(|e| {
            format!("cannot undo {tool}: failed to back up the current {original} first ({e})")
        })?;
        fs::copy(original_p, &backup).map_err(|e| {
            format!("cannot undo {tool}: failed to back up the current {original} first ({e})")
        })?;
        backed_up = Some(backup.display().to_string());
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
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = marker {
        eprintln!(
            "transcript: undo marker write failed ({e}) — a repeat /undo will \
             re-restore the same entry"
        );
    }

    Ok(match backed_up {
        Some(b) => format!(
            "restored {original} from trash (undid {tool}; the trash copy at {trash} is kept). \
             The content that was there is backed up at {b}."
        ),
        None => format!(
            "restored {original} from trash (undid {tool}; the trash copy at {trash} is kept)"
        ),
    })
}

/// Render the cumulative diff of the **last turn** — for `/diff`. For each
/// entry of the most recent turn that carries a pre-image, diffs the trashed
/// pre-image (before) against the file now on disk (after), via the same
/// colored renderer the `[y/N]` edit gate uses. Read-only: no restore, no
/// markers, no shadow-git. A `move_to_trash` deletion shows as an all-`-`
/// block (the file is gone from disk); an overwrite shows the real hunk.
///
/// Limitation: brand-new file *creates* carry no pre-image (nothing was
/// snapshotted), so they don't appear — `/diff` covers overwrites and
/// deletes, which is what the recoverability machinery tracks.
pub fn diff_last_turn() -> Result<String, String> {
    let entries = fs::read_to_string(transcript_path())
        .map_err(|_| "no actions recorded yet — nothing to diff".to_string())?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect::<Vec<_>>();

    // Last turn = the turn id on the most recent entry that carries one.
    let turn = entries
        .iter()
        .rev()
        .find_map(|e| e.get("turn").and_then(Value::as_str).map(str::to_string))
        .ok_or_else(|| "no turn-tagged actions recorded yet — nothing to diff".to_string())?;

    // That turn's pre-image-bearing entries, in reading (oldest-first) order.
    let group: Vec<&Value> = entries
        .iter()
        .filter(|e| e.get("turn").and_then(Value::as_str) == Some(turn.as_str()))
        .filter(|e| e.get("undo").and_then(|u| u.get("trash")).is_some())
        .collect();
    if group.is_empty() {
        return Err(
            "the last turn changed no files with a recoverable pre-image \
                    (brand-new files aren't tracked by /diff)"
                .to_string(),
        );
    }

    let mut blocks = Vec::new();
    for e in group {
        let undo_ref = e.get("undo").cloned().unwrap_or(Value::Null);
        let (Some(trash), Some(original)) = (
            undo_ref.get("trash").and_then(Value::as_str),
            undo_ref.get("original").and_then(Value::as_str),
        ) else {
            continue;
        };
        // before = the trashed pre-image; after = the file now on disk (empty
        // when the action deleted it, i.e. move_to_trash left nothing behind).
        let before = fs::read_to_string(trash).unwrap_or_default();
        let after = fs::read_to_string(original).unwrap_or_default();
        if before == after {
            continue;
        }
        blocks.push(crate::diff_preview::render_file_change(original, &before, &after).join("\n"));
    }
    if blocks.is_empty() {
        return Err(
            "the last turn's changes are no longer on disk (reverted or already undone)"
                .to_string(),
        );
    }
    Ok(blocks.join("\n\n"))
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
    fn undo_never_destroys_newer_content_at_the_original_path() {
        // CRITICAL (final roast): overwrite a file (old → trash), then the
        // user edits it again (newer content), then /undo. Undo must restore
        // the trashed version WITHOUT losing the newer edit — the newer
        // content must itself be backed up to trash first.
        with_temp_home(|home| {
            let f = write_tmp(home, "doc.txt", "ORIGINAL");
            // Simulate an overwrite: snapshot the original, then write v2.
            let _ = snapshot_to_trash(&f).unwrap();
            record("write_file", "doc.txt v2", take_pending_undo());
            fs::write(&f, "VERSION_TWO").unwrap();
            // User then hand-edits to a THIRD, newer version.
            fs::write(&f, "VERSION_THREE_NEWEST").unwrap();

            let msg = undo_last().expect("undo should succeed");
            // Restored to the snapshotted ORIGINAL...
            assert_eq!(fs::read_to_string(&f).unwrap(), "ORIGINAL");
            // ...and the newest content is NOT gone — it's backed up in trash.
            assert!(
                msg.contains("backed up"),
                "undo must report the pre-restore backup: {msg}"
            );
            let trash = home.join(".claudette").join("trash");
            let recovered = std::fs::read_dir(&trash)
                .unwrap()
                .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap_or_default())
                .any(|c| c == "VERSION_THREE_NEWEST");
            assert!(
                recovered,
                "the clobbered newer content must survive in trash"
            );
        });
    }

    /// Append a hand-forged transcript line — simulates a tampered
    /// actions.jsonl (the file is plain user-writable JSON).
    fn forge_entry(trash: &Path, original: &Path) {
        let line = json!({
            "ts": 1u64,
            "tool": "write_file",
            "input": "forged",
            "undo": {
                "trash": trash.display().to_string(),
                "original": original.display().to_string(),
            },
        });
        let path = transcript_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    #[test]
    fn undo_refuses_restore_target_outside_allowed_roots() {
        with_temp_home(|home| {
            // CLAUDETTE_WORKSPACE may be inherited from the dev environment —
            // pin it away so the forged target is outside every allowed root.
            let prev_ws = std::env::var("CLAUDETTE_WORKSPACE").ok();
            std::env::remove_var("CLAUDETTE_WORKSPACE");

            // A real trash copy (legitimate source) ...
            let victim = write_tmp(home, "victim.txt", "X");
            let trash = snapshot_to_trash(&victim).unwrap();
            let _ = take_pending_undo();
            // ... but a forged restore target OUTSIDE home/workspace.
            let outside = home
                .parent()
                .unwrap()
                .join("claudette-forged-restore-target.txt");
            forge_entry(&trash, &outside);

            let err = undo_last().unwrap_err();
            assert!(err.contains("path validation"), "got: {err}");
            assert!(!outside.exists(), "forged target must not be written");

            match prev_ws {
                Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
                None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
            }
        });
    }

    #[test]
    fn undo_refuses_trash_source_outside_trash_dir() {
        with_temp_home(|home| {
            // Forged ref: the source is a real, readable file OUTSIDE the
            // trash (a credential stand-in) — /undo must not become an
            // arbitrary file-copy primitive.
            fs::create_dir_all(trash_dir()).unwrap();
            let secret = write_tmp(home, "secret-standin.txt", "PRIVATE");
            let dest = home.join("leak.txt");
            forge_entry(&secret, &dest);

            let err = undo_last().unwrap_err();
            assert!(err.contains("outside the trash dir"), "got: {err}");
            assert!(!dest.exists(), "forged copy must not happen");
        });
    }

    #[test]
    fn undo_restores_workspace_files_outside_home() {
        // The validation must NOT be a naive home-only check: a legit undo of
        // a CLAUDETTE_WORKSPACE file overwrite targets a path outside $HOME.
        with_temp_home(|home| {
            let prev_ws = std::env::var("CLAUDETTE_WORKSPACE").ok();
            let ws = home
                .parent()
                .unwrap()
                .join(format!("claudette-undo-ws-{}", std::process::id()));
            fs::create_dir_all(&ws).unwrap();
            std::env::set_var("CLAUDETTE_WORKSPACE", &ws);

            let f = ws.join("project.txt");
            fs::write(&f, "OLD").unwrap();
            let _ = snapshot_to_trash(&f).unwrap();
            record("write_file", "project.txt", take_pending_undo());
            fs::write(&f, "NEW").unwrap();

            let msg = undo_last().expect("workspace undo must stay allowed");
            assert_eq!(fs::read_to_string(&f).unwrap(), "OLD", "{msg}");

            match prev_ws {
                Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
                None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
            }
            let _ = fs::remove_dir_all(&ws);
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
    fn record_redacts_secrets_before_writing_to_disk() {
        // Wave 1.3: a PAT pasted into a tool argument must never reach the
        // plaintext transcript file on disk.
        with_temp_home(|_| {
            record(
                "run_bash",
                r#"{"command":"git push https://ghp_ABCDEFGHIJKLMNOP0123456789@github.com/o/r"}"#,
                None,
            );
            let raw = fs::read_to_string(transcript_path()).unwrap();
            assert!(
                !raw.contains("ghp_ABCDEFGHIJKLMNOP"),
                "the PAT must not be persisted: {raw}"
            );
            assert!(
                raw.contains("redacted"),
                "a redaction marker should remain: {raw}"
            );
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

    #[test]
    fn undo_last_turn_restores_every_action_of_the_turn() {
        with_temp_home(|home| {
            // One turn deletes two files.
            begin_turn();
            let a = write_tmp(home, "a.md", "AAA");
            let b = write_tmp(home, "b.md", "BBB");
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());
            let _ = move_to_trash(&b).unwrap();
            record("note_delete", "b", take_pending_undo());

            let msg = undo_last_turn().expect("undo whole turn");
            assert!(a.exists() && b.exists(), "both files restored: {msg}");
            assert_eq!(fs::read_to_string(&a).unwrap(), "AAA");
            assert_eq!(fs::read_to_string(&b).unwrap(), "BBB");

            let err = undo_last_turn().unwrap_err();
            assert!(err.contains("nothing to undo"), "got: {err}");
        });
    }

    #[test]
    fn undo_last_turn_leaves_earlier_turns_intact() {
        with_temp_home(|home| {
            begin_turn();
            let a = write_tmp(home, "a.md", "AAA");
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());

            begin_turn(); // a distinct, later turn
            let b = write_tmp(home, "b.md", "BBB");
            let _ = move_to_trash(&b).unwrap();
            record("note_delete", "b", take_pending_undo());

            let msg = undo_last_turn().expect("undo last turn");
            assert!(b.exists(), "last turn's file restored: {msg}");
            assert!(!a.exists(), "the earlier turn must stay untouched: {msg}");

            // A second whole-turn undo reaches back to the prior turn.
            let msg2 = undo_last_turn().expect("undo prior turn");
            assert!(a.exists(), "prior turn now restored: {msg2}");
        });
    }

    #[test]
    fn undo_one_reverts_a_single_action_within_a_multi_action_turn() {
        with_temp_home(|home| {
            begin_turn();
            let a = write_tmp(home, "a.md", "AAA");
            let b = write_tmp(home, "b.md", "BBB");
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());
            let _ = move_to_trash(&b).unwrap();
            record("note_delete", "b", take_pending_undo());

            // `/undo one` == undo_last: only the most recent action (b).
            let msg = undo_last().expect("undo one");
            assert!(b.exists(), "b restored: {msg}");
            assert!(
                !a.exists(),
                "a stays deleted after a single-step undo: {msg}"
            );
        });
    }

    #[test]
    fn undo_last_turn_without_turn_ids_falls_back_to_single_entry() {
        with_temp_home(|home| {
            // No begin_turn() — entries carry turn: null, so the turn-scoped
            // undo degrades to reverting just the newest recoverable action.
            let a = write_tmp(home, "a.md", "AAA");
            let b = write_tmp(home, "b.md", "BBB");
            let _ = move_to_trash(&a).unwrap();
            record("note_delete", "a", take_pending_undo());
            let _ = move_to_trash(&b).unwrap();
            record("note_delete", "b", take_pending_undo());

            let msg = undo_last_turn().expect("fallback undo");
            assert!(b.exists() && !a.exists(), "only the newest restored: {msg}");
        });
    }

    #[test]
    fn diff_last_turn_renders_the_turns_changes() {
        with_temp_home(|home| {
            begin_turn();
            let f = write_tmp(home, "doc.txt", "line one\nOLD\nline three\n");
            let _ = snapshot_to_trash(&f).unwrap();
            record("write_file", "doc.txt", take_pending_undo());
            fs::write(&f, "line one\nNEW\nline three\n").unwrap();

            let diff = diff_last_turn().expect("diff should render");
            assert!(diff.contains("doc.txt"), "header missing: {diff}");
            assert!(diff.contains("- OLD"), "removal missing: {diff}");
            assert!(diff.contains("+ NEW"), "addition missing: {diff}");
            // Unchanged lines stay dim context, never `-`/`+` markers.
            assert!(
                !diff.contains("- line one"),
                "context wrongly marked: {diff}"
            );
        });
    }

    #[test]
    fn diff_last_turn_covers_only_the_last_turn() {
        with_temp_home(|home| {
            // Turn 1 overwrites f1; turn 2 overwrites f2. /diff shows only f2.
            begin_turn();
            let f1 = write_tmp(home, "one.txt", "V1\n");
            let _ = snapshot_to_trash(&f1).unwrap();
            record("write_file", "one.txt", take_pending_undo());
            fs::write(&f1, "V1b\n").unwrap();

            begin_turn();
            let f2 = write_tmp(home, "two.txt", "V2\n");
            let _ = snapshot_to_trash(&f2).unwrap();
            record("write_file", "two.txt", take_pending_undo());
            fs::write(&f2, "V2b\n").unwrap();

            let diff = diff_last_turn().expect("diff should render");
            assert!(diff.contains("two.txt"), "last turn's file shown: {diff}");
            assert!(
                !diff.contains("one.txt"),
                "earlier turn must not appear: {diff}"
            );
        });
    }

    #[test]
    fn diff_last_turn_with_no_pre_images_is_explicit() {
        with_temp_home(|_| {
            begin_turn();
            // A mutating action that carries no pre-image (e.g. todo_add).
            record("todo_add", "whatever", None);
            let err = diff_last_turn().unwrap_err();
            assert!(err.contains("no files"), "got: {err}");
        });
    }
}
