//! Brownfield missions group — 5 tools wrapping the
//! `clone → edit → submit-PR` workflow against external repos.
//!
//! State lives in `crate::missions`; this module is the user-facing
//! tool surface. Design (locked 2026-05-09):
//!
//! - **Implicit cwd:** the brain doesn't pass `mission_slug` per call.
//!   Every `git_*`, `bash`, `edit_file`, `read_file`, `write_file`,
//!   `glob_search`, `grep_search` call resolves against the active
//!   mission tree while a mission is active. Step 2 of T2 wired that.
//! - **One at a time:** trying to start a second mission errors.
//! - **A-lite persistence:** missions survive restart on disk via the
//!   JSON marker, but auto-attach is intentionally NOT done — the user
//!   opts in via a future `mission_attach` (deferred from T2 first cut).
//! - **Sandbox auto-extends** to the active mission tree for write_file
//!   (step 3 wired that in `validate_write_path`).
//! - **`mission_submit` auto-branches** off `main`/`master` to
//!   `claudette-mission/<slug>` before staging, mirroring clawForge's
//!   `github_pr.rs::create_pr` ergonomics.

use serde_json::{json, Value};

use super::{extract_str, parse_json_input};
use crate::missions::{
    active_mission, clear_active, list_missions, missions_root, save_marker, set_active,
    validate_slug, Mission,
};
use crate::test_runner::run_command_with_timeout;

const CLONE_TIMEOUT_SECS: u64 = 120;
const GIT_TIMEOUT_SECS: u64 = 30;
const PUSH_TIMEOUT_SECS: u64 = 60;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "mission_start",
                "description": "Clone a brownfield repo into ~/.claudette/missions/<slug>/ and make it the session's active mission. While active, git_*, bash, edit_file, read_file, write_file, and search calls run inside the mission tree. Use mission_exit to clear.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Either 'owner/repo' (assumed GitHub via https) or a full git URL (https://, http://, git@, ssh://)" },
                        "dest":   { "type": "string", "description": "Optional dest slug under ~/.claudette/missions/. Defaults to the repo name." }
                    },
                    "required": ["target"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mission_status",
                "description": "Show the currently active mission (slug, path, GitHub repo, current branch). Returns null when no mission is active.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mission_list",
                "description": "List every mission registered under ~/.claudette/missions/ (active or not).",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mission_exit",
                "description": "Clear the active mission. The cloned tree is left intact for resumption.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mission_submit",
                "description": "Capstone: stage all changes (add -A), commit, push -u, and open a PR against the mission's GitHub repo. Auto-creates 'claudette-mission/<slug>' branch if currently on main/master.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title":       { "type": "string",  "description": "PR title (also used as commit message subject)" },
                        "body":        { "type": "string",  "description": "PR/commit body (Markdown). Optional." },
                        "fixes_issue": { "type": "number",  "description": "Issue number to auto-close via 'Fixes #N'. Optional." },
                        "draft":       { "type": "boolean", "description": "Open as draft (default: false)" }
                    },
                    "required": ["title"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let r = match name {
        "mission_start" => run_mission_start(input),
        "mission_status" => run_mission_status(),
        "mission_list" => run_mission_list(),
        "mission_exit" => run_mission_exit(),
        "mission_submit" => run_mission_submit(input),
        _ => return None,
    };
    Some(r)
}

// ─── target parsing ──────────────────────────────────────────────────────

/// Outcome of parsing a `mission_start` target string.
#[derive(Debug)]
struct ParsedTarget {
    clone_url: String,
    /// Canonical `owner/repo` if the target points at GitHub, else `None`.
    /// Used by `mission_submit` to know which API repo to PR against.
    repo: Option<String>,
    /// Default slug if the user didn't pass `dest` — derived from the repo
    /// name (last URL segment minus `.git`).
    default_dest: String,
}

fn parse_target(target: &str) -> Result<ParsedTarget, String> {
    let t = target.trim();
    if t.is_empty() {
        return Err("mission_start: empty target".to_string());
    }

    // Bare `owner/repo` — assume GitHub https. Reject anything that looks
    // like a path with traversal or extra segments to keep the surface small.
    if !t.contains("://") && !t.contains('@') && !t.contains(' ') {
        let parts: Vec<&str> = t.split('/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            let owner = parts[0];
            let repo_name = parts[1].trim_end_matches(".git");
            if owner.contains("..") || repo_name.contains("..") {
                return Err(format!("mission_start: invalid owner/repo: {t}"));
            }
            let canonical = format!("{owner}/{repo_name}");
            return Ok(ParsedTarget {
                clone_url: format!("https://github.com/{canonical}.git"),
                repo: Some(canonical),
                default_dest: repo_name.to_string(),
            });
        }
    }

    // Full URL form. Validate scheme up front; canonicalise GitHub forms.
    let scheme_ok = t.starts_with("https://")
        || t.starts_with("http://")
        || t.starts_with("git@")
        || t.starts_with("ssh://");
    if !scheme_ok {
        return Err(format!(
            "mission_start: unsupported target — must be 'owner/repo' or a git URL (https://, http://, git@, ssh://), got `{t}`"
        ));
    }

    Ok(ParsedTarget {
        clone_url: t.to_string(),
        repo: parse_github_canonical(t),
        default_dest: derive_dest_from_url(t),
    })
}

fn parse_github_canonical(url: &str) -> Option<String> {
    let stripped = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let stripped = stripped.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = stripped.splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn derive_dest_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo")
        .to_string()
}

// ─── mission_start ───────────────────────────────────────────────────────

fn run_mission_start(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "mission_start")?;
    let target = extract_str(&v, "target", "mission_start")?;

    if let Some(active) = active_mission() {
        return Err(format!(
            "mission_start: '{}' is already active — exit it first with mission_exit",
            active.slug
        ));
    }

    let parsed = parse_target(target)?;
    let dest_raw = v
        .get("dest")
        .and_then(Value::as_str)
        .unwrap_or(&parsed.default_dest);
    let dest = validate_slug(dest_raw)?;

    let root = missions_root();
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("mission_start: create {} failed: {e}", root.display()))?;

    let target_path = root.join(&dest);
    if target_path.exists() {
        return Err(format!(
            "mission_start: target already exists at {} — pick a different dest or remove it first",
            target_path.display()
        ));
    }
    let target_str = target_path
        .to_str()
        .ok_or_else(|| {
            format!(
                "mission_start: target path is not utf-8: {}",
                target_path.display()
            )
        })?
        .to_string();

    let git_exe = super::git::resolve_git_path();
    let args: Vec<&str> = vec!["clone", "--", &parsed.clone_url, &target_str];
    let result = run_command_with_timeout(&git_exe, &args, CLONE_TIMEOUT_SECS, None);
    if result.timed_out {
        let _ = std::fs::remove_dir_all(&target_path);
        return Err(format!(
            "mission_start: timed out after {CLONE_TIMEOUT_SECS}s for {}",
            parsed.clone_url
        ));
    }
    if !result.success {
        let _ = std::fs::remove_dir_all(&target_path);
        let stderr = if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            result.stderr.clone()
        };
        return Err(format!(
            "mission_start: clone failed (exit {:?}): {}",
            result.exit_code,
            stderr.chars().take(500).collect::<String>()
        ));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    let mission = Mission {
        slug: dest.clone(),
        path: target_path.clone(),
        repo: parsed.repo.clone(),
        created_at: now,
    };
    save_marker(&mission)?;
    set_active(mission)?;

    Ok(json!({
        "ok": true,
        "slug": dest,
        "path": target_str,
        "repo": parsed.repo,
        "url": parsed.clone_url,
        "note": "mission active — git_*, bash, edit_file, read_file, write_file, and search now run in this tree",
    })
    .to_string())
}

// ─── mission_status / mission_list / mission_exit ────────────────────────

// dispatch() needs every per-tool handler to share the same
// `Result<String, String>` shape, even infallible ones. Allowing the lint
// keeps the dispatch map uniform.
#[allow(clippy::unnecessary_wraps)]
fn run_mission_status() -> Result<String, String> {
    match active_mission() {
        None => Ok(json!({ "active": null }).to_string()),
        Some(m) => {
            let branch = current_branch_in(&m.path).unwrap_or_else(|_| "?".to_string());
            Ok(json!({
                "active": {
                    "slug": m.slug,
                    "path": m.path.display().to_string(),
                    "repo": m.repo,
                    "branch": branch,
                    "created_at": m.created_at,
                }
            })
            .to_string())
        }
    }
}

fn run_mission_list() -> Result<String, String> {
    let missions = list_missions()?;
    let active_slug = active_mission().map(|m| m.slug);
    let items: Vec<Value> = missions
        .iter()
        .map(|m| {
            json!({
                "slug": m.slug,
                "path": m.path.display().to_string(),
                "repo": m.repo,
                "active": active_slug.as_deref() == Some(&m.slug),
                "created_at": m.created_at,
            })
        })
        .collect();
    Ok(json!({ "count": items.len(), "items": items }).to_string())
}

fn run_mission_exit() -> Result<String, String> {
    match clear_active() {
        Some(slug) => Ok(json!({ "ok": true, "exited": slug }).to_string()),
        None => Err("mission_exit: no active mission".to_string()),
    }
}

// ─── mission_submit (capstone) ───────────────────────────────────────────

fn run_mission_submit(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "mission_submit")?;
    let title = extract_str(&v, "title", "mission_submit")?;
    let body_in = v.get("body").and_then(Value::as_str).unwrap_or("");
    let fixes_issue = v.get("fixes_issue").and_then(Value::as_u64);
    let draft = v.get("draft").and_then(Value::as_bool).unwrap_or(false);

    let mission =
        active_mission().ok_or("mission_submit: no active mission — run mission_start first")?;
    let repo = mission
        .repo
        .clone()
        .ok_or("mission_submit: this mission was not started from a GitHub repo, so PR creation is not supported. Use git_push + the lower-level GitHub tools manually.")?;
    let (owner, repo_name) = repo.split_once('/').ok_or_else(|| {
        format!("mission_submit: malformed repo identifier `{repo}` (expected owner/repo)")
    })?;

    // 1. Confirm there's something to commit. `git status --porcelain` is
    //    empty iff working tree + index are clean, in which case we should
    //    refuse rather than push an empty branch.
    let porcelain = git_in(&mission.path, &["status", "--porcelain"], GIT_TIMEOUT_SECS)?;
    if porcelain.trim().is_empty() {
        return Err(
            "mission_submit: working tree clean — nothing to commit. Edit some files first."
                .to_string(),
        );
    }

    // 2. Auto-branch off main/master before staging. The brain might have
    //    been editing on the default branch; pushing a feature branch is
    //    nicer for review and avoids attempting an upstream-default push
    //    the user almost certainly can't do anyway.
    let starting_branch = current_branch_in(&mission.path)?;
    let branch = if matches!(starting_branch.as_str(), "main" | "master") {
        let new_branch = format!("claudette-mission/{}", mission.slug);
        git_in(
            &mission.path,
            &["checkout", "-b", &new_branch],
            GIT_TIMEOUT_SECS,
        )?;
        new_branch
    } else {
        starting_branch
    };

    // 3. Stage everything. Inside a freshly cloned brownfield tree this is
    //    safe: there's no .venv noise or dotfile churn that the workspace
    //    `git_add` tool's `-A` ban was guarding against. The mission was
    //    created from a clean clone — anything that's modified is a real
    //    edit the brain made on purpose.
    git_in(&mission.path, &["add", "-A"], GIT_TIMEOUT_SECS)?;

    // 4. Build the commit message: title, blank line, body, and a "Fixes #N"
    //    trailer if requested. The same string is reused as the PR body so
    //    we don't have to re-derive it.
    use std::fmt::Write as _;
    let mut body_full = body_in.to_string();
    if let Some(num) = fixes_issue {
        if !body_full.is_empty() {
            body_full.push_str("\n\n");
        }
        let _ = write!(body_full, "Fixes #{num}");
    }
    let commit_msg = if body_full.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{body_full}")
    };
    git_in(
        &mission.path,
        &["commit", "-m", &commit_msg],
        GIT_TIMEOUT_SECS,
    )?;

    // 5. Push -u origin <branch>. If the user lacks push access this errors
    //    with whatever git printed; the brain reads the error and can
    //    suggest gh_fork + manual push to the brain's caller.
    git_in(
        &mission.path,
        &["push", "-u", "origin", &branch],
        PUSH_TIMEOUT_SECS,
    )?;

    // 6. Open the PR via the existing gh_create_pr tool. Reusing the tool
    //    dispatcher keeps auth/error/wrap-untrusted handling in one place.
    let pr_input = json!({
        "owner": owner,
        "repo": repo_name,
        "title": title,
        "body": body_full,
        "head": branch,
        "base": "main",
        "draft": draft,
    })
    .to_string();
    let pr_response = match crate::tools::dispatch_tool("gh_create_pr", &pr_input) {
        Ok(s) => s,
        Err(e) => {
            // Common case: the upstream's default branch is `master`, not
            // `main`. Retry once before giving up.
            if e.contains("base") || e.contains("master") {
                let retry = json!({
                    "owner": owner,
                    "repo": repo_name,
                    "title": title,
                    "body": body_full,
                    "head": branch,
                    "base": "master",
                    "draft": draft,
                })
                .to_string();
                crate::tools::dispatch_tool("gh_create_pr", &retry)?
            } else {
                return Err(e);
            }
        }
    };
    let pr: Value = serde_json::from_str(&pr_response)
        .map_err(|e| format!("mission_submit: gh_create_pr returned non-json: {e}"))?;

    Ok(json!({
        "ok": true,
        "slug": mission.slug,
        "branch": branch,
        "pr_number": pr.get("number"),
        "pr_url": pr.get("url"),
        "draft": draft,
    })
    .to_string())
}

// ─── helpers ─────────────────────────────────────────────────────────────

/// Run `git <args>` inside an explicit cwd. Used by `mission_submit` so the
/// commands target the mission tree even outside the active-mission cwd
/// path (defensive — the active mission *is* the same tree, but pinning
/// here means the tool keeps working if a future refactor changes how the
/// active cwd is derived).
fn git_in(cwd: &std::path::Path, args: &[&str], timeout: u64) -> Result<String, String> {
    let git_exe = super::git::resolve_git_path();
    let result = run_command_with_timeout(&git_exe, args, timeout, Some(cwd));
    if result.timed_out {
        return Err(format!(
            "mission: git {} timed out after {timeout}s",
            args.first().unwrap_or(&"")
        ));
    }
    if !result.success {
        let stderr = if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            result.stderr.clone()
        };
        return Err(format!(
            "mission: git {} failed (exit {:?}): {}",
            args.first().unwrap_or(&""),
            result.exit_code,
            stderr.chars().take(500).collect::<String>()
        ));
    }
    Ok(result.stdout)
}

fn current_branch_in(cwd: &std::path::Path) -> Result<String, String> {
    let out = git_in(
        cwd,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        GIT_TIMEOUT_SECS,
    )?;
    Ok(out.trim().to_string())
}

// ─── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_target_owner_repo_form() {
        let p = parse_target("octocat/Hello-World").unwrap();
        assert_eq!(p.clone_url, "https://github.com/octocat/Hello-World.git");
        assert_eq!(p.repo.as_deref(), Some("octocat/Hello-World"));
        assert_eq!(p.default_dest, "Hello-World");
    }

    #[test]
    fn parse_target_owner_repo_strips_dot_git() {
        let p = parse_target("octocat/Hello-World.git").unwrap();
        assert_eq!(p.clone_url, "https://github.com/octocat/Hello-World.git");
        assert_eq!(p.default_dest, "Hello-World");
    }

    #[test]
    fn parse_target_https_github() {
        let p = parse_target("https://github.com/octocat/Hello-World.git").unwrap();
        assert_eq!(p.repo.as_deref(), Some("octocat/Hello-World"));
        assert_eq!(p.default_dest, "Hello-World");
    }

    #[test]
    fn parse_target_https_github_no_dot_git() {
        let p = parse_target("https://github.com/octocat/Hello-World").unwrap();
        assert_eq!(p.repo.as_deref(), Some("octocat/Hello-World"));
        assert_eq!(p.default_dest, "Hello-World");
    }

    #[test]
    fn parse_target_ssh_github() {
        let p = parse_target("git@github.com:octocat/Hello-World.git").unwrap();
        assert_eq!(p.repo.as_deref(), Some("octocat/Hello-World"));
        assert_eq!(p.default_dest, "Hello-World");
    }

    #[test]
    fn parse_target_non_github_url_keeps_repo_none() {
        let p = parse_target("https://gitlab.com/group/proj.git").unwrap();
        assert!(p.repo.is_none());
        assert_eq!(p.default_dest, "proj");
    }

    #[test]
    fn parse_target_rejects_unsupported_scheme() {
        let err = parse_target("file:///etc/passwd").unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn parse_target_rejects_empty() {
        assert!(parse_target("").is_err());
        assert!(parse_target("   ").is_err());
    }

    #[test]
    fn parse_target_rejects_traversal_in_owner_repo() {
        assert!(parse_target("../etc/passwd").is_err());
        assert!(parse_target("foo/../bar").is_err());
    }

    #[test]
    fn parse_target_rejects_three_slash_form() {
        // `owner/repo/extra` is ambiguous — refuse rather than guess.
        let err = parse_target("octocat/Hello-World/branch").unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn derive_dest_from_url_handles_trailing_slash_and_dot_git() {
        assert_eq!(
            derive_dest_from_url("https://example.com/foo/bar.git"),
            "bar"
        );
        assert_eq!(
            derive_dest_from_url("https://example.com/foo/bar.git/"),
            "bar"
        );
        assert_eq!(derive_dest_from_url("git@host:owner/repo"), "repo");
    }

    #[test]
    fn parse_github_canonical_recognises_known_forms() {
        for url in [
            "https://github.com/octocat/Hello-World.git",
            "https://github.com/octocat/Hello-World",
            "http://github.com/octocat/Hello-World",
            "git@github.com:octocat/Hello-World.git",
            "ssh://git@github.com/octocat/Hello-World.git",
        ] {
            assert_eq!(
                parse_github_canonical(url).as_deref(),
                Some("octocat/Hello-World"),
                "wrong canonical for {url}"
            );
        }
    }

    #[test]
    fn parse_github_canonical_returns_none_for_other_hosts() {
        assert!(parse_github_canonical("https://gitlab.com/group/proj").is_none());
        assert!(parse_github_canonical("https://bitbucket.org/owner/repo").is_none());
    }

    #[test]
    fn run_mission_start_rejects_missing_target() {
        // Don't actually start a mission — just verify input validation.
        let err = run_mission_start("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn run_mission_submit_rejects_missing_title() {
        let err = run_mission_submit("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn run_mission_submit_rejects_when_no_active_mission() {
        // No mission active in the test process → should error cleanly
        // rather than panic. (If this test ever runs after another test
        // that left a mission active, the assertion shape still holds —
        // either "no active" or "no GitHub repo" / "clean" depending on
        // the leftover state. We accept any of those.)
        if active_mission().is_some() {
            return; // can't assert from here without disturbing other tests
        }
        let err = run_mission_submit(r#"{"title":"x"}"#).unwrap_err();
        assert!(err.contains("no active mission"), "got: {err}");
    }

    #[test]
    fn run_mission_exit_errors_when_nothing_active() {
        if active_mission().is_some() {
            return; // see comment above
        }
        let err = run_mission_exit().unwrap_err();
        assert!(err.contains("no active mission"), "got: {err}");
    }

    #[test]
    fn run_mission_status_returns_null_when_inactive() {
        if active_mission().is_some() {
            return;
        }
        let out = run_mission_status().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("active").is_some_and(Value::is_null));
    }

    #[test]
    fn run_mission_list_succeeds_even_when_root_absent() {
        // list_missions() returns Ok(empty) if missions_root doesn't exist;
        // run_mission_list wraps that. Don't assert count because the
        // user's real ~/.claudette/missions/ may have entries.
        let out = run_mission_list().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("count").is_some());
        assert!(v.get("items").and_then(Value::as_array).is_some());
    }

    #[test]
    fn schemas_lists_five_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "mission_start",
                "mission_status",
                "mission_list",
                "mission_exit",
                "mission_submit",
            ]
        );
    }
}
