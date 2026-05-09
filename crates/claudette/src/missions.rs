//! Brownfield missions — session-scoped active workspace pointing at a
//! cloned external repo under `~/.claudette/missions/<slug>/`.
//!
//! T1 (already shipped) gave the brain the 5 brownfield tools (`git_clone`,
//! `gh_list_repo_issues`, `gh_pr_status`, `gh_fork`, `gh_create_pr`) but
//! every shell/git/file tool still ran in claudette's launch cwd — so once
//! a repo was cloned, the brain couldn't drive `git_status`/`commit`/`push`
//! against the new tree from the same session. T2 closes that gap by
//! introducing a single piece of session state, the **active mission**.
//! While a mission is active, `tools::active_cwd()` resolves to the mission
//! root and the cwd-routing primitive in git/shell/file_ops/search uses
//! that instead of the workspace cwd.
//!
//! Persistence: a JSON marker file at
//! `~/.claudette/missions/<slug>/.claudette-mission.json` survives restart
//! so missions are recoverable, but **auto-attach is intentionally not
//! done** on startup — the user opts in via `mission_attach` (deferred from
//! T2 first cut) when they want to resume.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// On-disk marker filename, sat at the root of every mission tree.
pub const MARKER_FILENAME: &str = ".claudette-mission.json";

/// One brownfield mission: a slug pointing at a cloned tree on disk.
///
/// `repo` is the canonical `owner/repo` form when the mission was started
/// from a GitHub URL (so `mission_submit` knows where to open the PR);
/// `None` for missions cloned from a non-GitHub remote, in which case
/// `mission_submit` will refuse with a clear error rather than guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    pub slug: String,
    pub path: PathBuf,
    pub repo: Option<String>,
    pub created_at: i64,
}

/// Process-wide active-mission slot. `OnceLock<Mutex<…>>` rather than a
/// plain static so initialisation is lazy and lock acquisition is honest
/// about its potential to block (it won't, in practice — every callsite
/// holds the lock for microseconds).
fn active_slot() -> &'static Mutex<Option<Mission>> {
    static SLOT: OnceLock<Mutex<Option<Mission>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Resolve `~/.claudette/missions/`. Mirrors the home-resolution pattern in
/// `crate::secrets::secrets_dir` and `tools::git::missions_root`.
#[must_use]
pub fn missions_root() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("missions")
}

/// Validate a mission slug. Same rules as `git_clone`'s dest validator:
/// single path component, no `..`, no `/`, `\`, or `:` (Windows drive
/// prefix). Trims and returns the canonical form.
pub fn validate_slug(slug: &str) -> Result<String, String> {
    let trimmed = slug.trim();
    if trimmed.is_empty() {
        return Err("mission: empty slug".to_string());
    }
    if trimmed.contains("..") {
        return Err(format!("mission: slug may not contain '..' ({trimmed})"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(format!(
            "mission: slug must be a single directory name, not a path ({trimmed})"
        ));
    }
    if trimmed.contains(':') {
        return Err(format!("mission: slug may not contain ':' ({trimmed})"));
    }
    Ok(trimmed.to_string())
}

/// Snapshot the currently-active mission (clones out of the lock).
#[must_use]
pub fn active_mission() -> Option<Mission> {
    active_slot().lock().ok().and_then(|guard| guard.clone())
}

/// Return the cwd that subprocess-based tools (git, bash, etc.) should
/// run in. While a mission is active that's the mission root; otherwise
/// the process cwd (claudette's launch directory).
#[must_use]
pub fn active_cwd() -> PathBuf {
    if let Some(m) = active_mission() {
        return m.path;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Make `mission` the active one. One mission at a time per session
/// (design pick #2): if another mission is already active, errors so the
/// caller can surface "exit current mission first" to the brain.
pub fn set_active(mission: Mission) -> Result<(), String> {
    let mut guard = active_slot()
        .lock()
        .map_err(|_| "mission: active slot poisoned".to_string())?;
    if let Some(existing) = guard.as_ref() {
        return Err(format!(
            "mission: '{}' is already active — exit it first with mission_exit",
            existing.slug
        ));
    }
    *guard = Some(mission);
    Ok(())
}

/// Clear the active mission. Returns the slug that was cleared (for
/// caller-side logging); `None` if nothing was active.
pub fn clear_active() -> Option<String> {
    let mut guard = active_slot().lock().ok()?;
    guard.take().map(|m| m.slug)
}

/// Path of the on-disk marker for a given mission tree.
#[must_use]
pub fn marker_path_for(mission_path: &Path) -> PathBuf {
    mission_path.join(MARKER_FILENAME)
}

/// Write the JSON marker into the mission tree. Called by `mission_start`
/// right after a successful clone so the mission is recoverable.
pub fn save_marker(mission: &Mission) -> Result<(), String> {
    let path = marker_path_for(&mission.path);
    let json = serde_json::to_string_pretty(mission)
        .map_err(|e| format!("mission: serialize marker: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("mission: write {} failed: {e}", path.display()))?;
    Ok(())
}

/// Read a marker JSON back into a `Mission`. Used by `mission_list` and
/// (later) `mission_attach`.
pub fn load_marker(mission_path: &Path) -> Result<Mission, String> {
    let path = marker_path_for(mission_path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("mission: read {} failed: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("mission: parse {}: {e}", path.display()))
}

/// Enumerate every mission whose marker survives under
/// `~/.claudette/missions/`. Directories without a marker are skipped
/// (see `list_orphan_slugs` to surface those separately).
pub fn list_missions() -> Result<Vec<Mission>, String> {
    let root = missions_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let read = std::fs::read_dir(&root)
        .map_err(|e| format!("mission: read {} failed: {e}", root.display()))?;
    for entry in read.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        // Silently skip directories without a marker — listing should
        // never fail because of a single broken entry. `list_orphan_slugs`
        // gives the user a way to see what got skipped.
        if let Ok(m) = load_marker(&p) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// Sibling to [`list_missions`]: directory names under
/// `~/.claudette/missions/` that are *not* recognised as missions
/// because they lack a valid `.claudette-mission.json` marker. Common
/// causes: pre-T2 `git_clone` calls (markers didn't exist yet),
/// half-finished clones, or unrelated user content the user dropped
/// into the missions root by hand. Surfaced via `mission_list` so the
/// brain can warn rather than silently ignore them.
pub fn list_orphan_slugs() -> Result<Vec<String>, String> {
    let root = missions_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let read = std::fs::read_dir(&root)
        .map_err(|e| format!("mission: read {} failed: {e}", root.display()))?;
    for entry in read.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if load_marker(&p).is_err() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_slug_accepts_simple_name() {
        assert_eq!(
            validate_slug("django__issue-12345").unwrap(),
            "django__issue-12345"
        );
    }

    #[test]
    fn validate_slug_trims_whitespace() {
        assert_eq!(validate_slug("  hello  ").unwrap(), "hello");
    }

    #[test]
    fn validate_slug_rejects_traversal_and_separators() {
        for bad in ["..", "foo/../bar", "a/b", "a\\b", "C:\\evil", "", "   "] {
            assert!(validate_slug(bad).is_err(), "expected reject for `{bad}`");
        }
    }

    #[test]
    fn missions_root_under_home_or_userprofile() {
        let root = missions_root();
        let display = root.display().to_string();
        assert!(
            display.contains(".claudette") && display.ends_with("missions"),
            "unexpected missions_root: {display}"
        );
    }

    #[test]
    fn marker_round_trip() {
        // Round-trip a Mission through save_marker → load_marker. Uses a
        // temp dir under the OS temp root so we don't pollute the real
        // ~/.claudette/missions tree.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tmp = std::env::temp_dir().join(format!("claudette-mission-test-{nanos}"));
        std::fs::create_dir_all(&tmp).unwrap();

        let mission = Mission {
            slug: "abcc-cleanup".to_string(),
            path: tmp.clone(),
            repo: Some("mrdushidush/agent-battle-command-center".to_string()),
            created_at: 1_700_000_000,
        };

        save_marker(&mission).expect("save_marker should succeed");
        let loaded = load_marker(&tmp).expect("load_marker should succeed");
        assert_eq!(loaded, mission);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_marker_errors_on_missing_file() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tmp = std::env::temp_dir().join(format!("claudette-mission-missing-{nanos}"));
        std::fs::create_dir_all(&tmp).unwrap();

        let err = load_marker(&tmp).expect_err("expected error on missing marker");
        assert!(err.contains("read") || err.contains("failed"), "got: {err}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_missions_skips_dirs_without_markers() {
        // We can't easily redirect missions_root() in-process, so this test
        // just verifies the function runs cleanly and returns Ok for
        // whatever the real filesystem currently has — any listed entries
        // must have valid markers.
        let out = list_missions().expect("list_missions should not fail");
        for m in &out {
            assert!(!m.slug.is_empty(), "listed mission has empty slug");
        }
    }

    #[test]
    fn list_orphan_slugs_runs_cleanly() {
        // Same constraint as list_missions_skips_dirs_without_markers:
        // can't redirect missions_root() in-process, so just verify the
        // function returns Ok against the real filesystem and that
        // missions + orphans don't overlap.
        let orphans = list_orphan_slugs().expect("list_orphan_slugs should not fail");
        for s in &orphans {
            assert!(!s.is_empty(), "orphan with empty name");
        }
        let missions = list_missions().expect("list_missions should not fail");
        for m in &missions {
            assert!(
                !orphans.contains(&m.slug),
                "{} listed as both mission and orphan",
                m.slug
            );
        }
    }

    #[test]
    fn active_cwd_falls_back_to_process_cwd_when_no_mission() {
        // When nothing is active, active_cwd() must return the process cwd
        // — this is the fallback every cwd-routed tool relies on.
        // We don't mutate the active slot here because other tests may run
        // in parallel; instead we just snapshot and assert it's a real
        // directory the process can stat.
        if active_mission().is_none() {
            let cwd = active_cwd();
            assert!(
                cwd.is_dir(),
                "active_cwd fallback {} not a directory",
                cwd.display()
            );
        }
    }

    #[test]
    fn marker_path_for_appends_filename() {
        let p = Path::new("/tmp/foo");
        assert_eq!(marker_path_for(p), p.join(MARKER_FILENAME));
    }

    #[test]
    fn marker_filename_is_dotfile() {
        // Belt-and-braces: the marker is intentionally a dotfile so it
        // doesn't show up in `ls` of the cloned tree, doesn't get added
        // by accident via `git add <path>`, and is harmless if committed.
        assert!(MARKER_FILENAME.starts_with('.'));
    }
}
