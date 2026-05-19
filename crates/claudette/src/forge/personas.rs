//! Persona loader — bundled + user-defined.
//!
//! Originally ported from `claudettes-forge/crates/core/src/personas.rs` at
//! the `rc1-final` tag. Forge-mode Coder is wired against `codex7` since v0b;
//! assistant-mode (Eva) and Verifier (Sentinel-9) get wired in
//! `import_2026_05_19` Phase 2 alongside the `--faceless` flag.
//!
//! A persona bundles name, role, voice style, backstory, and example
//! interactions into a single markdown file (TOML frontmatter + markdown
//! body). At startup the loader walks two directories:
//!
//! 1. **Bundled personas** shipped inside the repo under
//!    `crates/claudette/personas/`.
//! 2. **User overrides** at `$PROJECT/.claudette/personas/`.
//!
//! User overrides win by filename — drop `codex7.md` into your project's
//! `.claudette/personas/` to rewrite CodeX-7 for that project. Restart is
//! required; no hot reload.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::Role;

/// A loaded persona, parsed from a markdown file with TOML frontmatter.
#[derive(Debug, Clone)]
pub struct Persona {
    /// Display name (e.g. "CodeX-7", "Eva").
    pub name: String,
    /// Which pipeline role this persona fills.
    pub role: Role,
    /// Voice style one-liner (e.g. "clipped-tactical", "warm-efficient").
    /// Passed to the model via system prompt.
    pub voice: String,
    /// Loading status — `Placeholder` means the markdown file had frontmatter
    /// but no body content yet; `Loaded` means backstory + examples are
    /// populated.
    pub status: PersonaStatus,
    /// Full backstory prose, injected verbatim into the system prompt.
    pub backstory: String,
    /// Worked examples showing the persona's preferred style + decision-making.
    pub examples: Vec<PersonaExample>,
}

/// A single worked example used in the system prompt. Typically 3-5 per persona.
#[derive(Debug, Clone)]
pub struct PersonaExample {
    /// Short label (shown in docs, not the model).
    pub label: String,
    /// Text to show the model.
    pub body: String,
}

/// Loading completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaStatus {
    /// Frontmatter parsed; body is still a placeholder.
    Placeholder,
    /// Frontmatter + body + examples all populated.
    Loaded,
    /// Sketch only — struct exists, content not yet written.
    Sketch,
}

/// A persona collection keyed by canonical name (lowercase, hyphen-free).
pub type PersonaMap = HashMap<String, Persona>;

/// Walk `bundled_dir` + `user_dir` and return the merged map.
///
/// User-dir entries override bundled ones by filename (case-insensitive).
/// Files that don't parse produce a warning on stderr and are skipped —
/// persona loading is best-effort, never load-bearing.
///
/// # Errors
/// Returns `Err` only when `bundled_dir` is missing. Individual parse
/// failures and a missing `user_dir` are logged and skipped.
pub fn load_personas(bundled_dir: &Path, user_dir: Option<&Path>) -> Result<PersonaMap, String> {
    let mut map = PersonaMap::new();
    load_dir_into(bundled_dir, &mut map)?;
    if let Some(ud) = user_dir {
        if ud.exists() {
            // User-dir errors are non-fatal — best-effort overlay.
            let _ = load_dir_into(ud, &mut map);
        }
    }
    Ok(map)
}

fn load_dir_into(dir: &Path, map: &mut PersonaMap) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("persona dir not found: {}", dir.display()));
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        match parse_persona_file(&path) {
            Ok(p) => {
                let key = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                map.insert(key, p);
            }
            Err(e) => {
                eprintln!("warn: persona {}: {e}", path.display());
            }
        }
    }
    Ok(())
}

/// Parse a single persona file. Public so tests can exercise it without
/// touching the filesystem walker.
///
/// # Errors
/// Returns `Err` when the file has no frontmatter, has malformed TOML,
/// declares an unknown `role`, or is empty.
pub fn parse_persona_file(path: &Path) -> Result<Persona, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_persona_content(&raw, &path.display().to_string())
}

/// Parse persona content from a raw string. Same parser as
/// [`parse_persona_file`], but the source isn't a real path — `label` is
/// used in error messages and can be anything that identifies the source
/// for a human reader (e.g. `"bundled:codex7"` for `include_str!`-baked
/// content).
///
/// # Errors
/// Returns `Err` when the content has no frontmatter, malformed TOML,
/// an unknown `role`, or is empty.
pub fn parse_persona_content(raw: &str, label: &str) -> Result<Persona, String> {
    // Normalise CRLF → LF so the "\n---" delimiter match and the TOML parser
    // both work on Windows checkouts (git autocrlf=true turns committed LF
    // into CRLF on disk).
    let raw = raw.replace("\r\n", "\n");

    // Strip leading "---" then match the closing "\n---" delimiter.
    let after_open = raw
        .strip_prefix("---")
        .ok_or_else(|| format!("{label}: missing leading --- frontmatter"))?
        .trim_start_matches('\n');
    let end_idx = after_open
        .find("\n---")
        .ok_or_else(|| format!("{label}: no closing --- after frontmatter"))?;
    let frontmatter = &after_open[..end_idx];
    let body = after_open[end_idx + "\n---".len()..].trim_start_matches('\n');

    let tbl: toml::Table =
        toml::from_str(frontmatter).map_err(|e| format!("{label}: toml parse: {e}"))?;

    let name = tbl
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{label}: missing 'name' field"))?
        .to_string();

    let role_str = tbl
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{label}: missing 'role' field"))?;
    let role = parse_role(role_str).ok_or_else(|| format!("{label}: unknown role '{role_str}'"))?;

    let voice = tbl
        .get("voice")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let status = tbl
        .get("status")
        .and_then(|v| v.as_str())
        .map_or(PersonaStatus::Placeholder, parse_status);

    let (backstory, examples) = split_body(body);

    Ok(Persona {
        name,
        role,
        voice,
        status,
        backstory,
        examples,
    })
}

fn parse_role(s: &str) -> Option<Role> {
    match s.to_lowercase().as_str() {
        "assistant" => Some(Role::Assistant),
        "planner" => Some(Role::Planner),
        "router" => Some(Role::Router),
        "coder" => Some(Role::Coder),
        "testcoder" | "test_coder" | "test-coder" => Some(Role::TestCoder),
        "verifier" => Some(Role::Verifier),
        "surgicalcoder" | "surgical_coder" | "surgical-coder" => Some(Role::SurgicalCoder),
        "cto" => Some(Role::Cto),
        _ => None,
    }
}

fn parse_status(s: &str) -> PersonaStatus {
    match s.to_lowercase().as_str() {
        "loaded" => PersonaStatus::Loaded,
        "sketch" => PersonaStatus::Sketch,
        _ => PersonaStatus::Placeholder,
    }
}

/// Separate the backstory from the `## Example moments` section if present.
fn split_body(body: &str) -> (String, Vec<PersonaExample>) {
    const MARKER: &str = "## Example moments";
    if let Some(idx) = body.find(MARKER) {
        let backstory = body[..idx].trim().to_string();
        let examples = parse_examples(&body[idx + MARKER.len()..]);
        (backstory, examples)
    } else {
        (body.trim().to_string(), Vec::new())
    }
}

fn parse_examples(section: &str) -> Vec<PersonaExample> {
    let mut out = Vec::new();
    let mut current_label: Option<String> = None;
    let mut current_body = String::new();
    for line in section.lines() {
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some(label) = current_label.take() {
                out.push(PersonaExample {
                    label,
                    body: current_body.trim().to_string(),
                });
                current_body.clear();
            }
            current_label = Some(rest.trim().to_string());
        } else if current_label.is_some() {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }
    if let Some(label) = current_label {
        out.push(PersonaExample {
            label,
            body: current_body.trim().to_string(),
        });
    }
    out
}

/// The default bundled-persona location relative to the binary cwd.
/// In dev this resolves to `personas/` at the workspace root.
#[must_use]
pub fn default_bundled_dir() -> PathBuf {
    PathBuf::from("personas")
}

/// The per-project user-override location.
/// `$PROJECT/.claudette/personas/`.
#[must_use]
pub fn default_user_dir(project_root: &Path) -> PathBuf {
    project_root.join(".claudette").join("personas")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a unique per-test temp dir under `claudette-forge-test-personas/`.
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("claudette-forge-test-personas")
            .join(format!(
                "{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    const VALID_PERSONA: &str = r#"---
name = "CodeX-7"
role = "coder"
voice = "clipped-tactical"
status = "placeholder"
---

Backstory prose goes here.

## Example moments

### Writing a function
Keep it short, no comments.

### Reviewing a diff
Flag anything over 50 lines.
"#;

    #[test]
    fn parse_valid_persona_file() {
        let dir = temp_dir("valid");
        let path = dir.join("codex7.md");
        write(&path, VALID_PERSONA);

        let p = parse_persona_file(&path).unwrap();
        assert_eq!(p.name, "CodeX-7");
        assert_eq!(p.role, Role::Coder);
        assert_eq!(p.voice, "clipped-tactical");
        assert_eq!(p.status, PersonaStatus::Placeholder);
        assert!(p.backstory.starts_with("Backstory prose"));
        assert_eq!(p.examples.len(), 2);
        assert_eq!(p.examples[0].label, "Writing a function");
        assert!(p.examples[0].body.contains("Keep it short"));
        assert_eq!(p.examples[1].label, "Reviewing a diff");
    }

    #[test]
    fn parse_persona_file_with_crlf_line_endings() {
        // Regression: Windows clones with git autocrlf=true turn committed LF
        // into CRLF on disk, which used to leave a trailing \r in the
        // frontmatter slice and fail TOML parsing.
        let dir = temp_dir("crlf");
        let path = dir.join("codex7.md");
        let crlf = VALID_PERSONA.replace('\n', "\r\n");
        write(&path, &crlf);
        let p = parse_persona_file(&path).unwrap();
        assert_eq!(p.name, "CodeX-7");
        assert_eq!(p.role, Role::Coder);
    }

    #[test]
    fn parse_missing_leading_marker_fails() {
        let dir = temp_dir("nomarker");
        let path = dir.join("x.md");
        write(&path, "name = \"x\"\nrole = \"coder\"\n---\nbody");
        let err = parse_persona_file(&path).unwrap_err();
        assert!(err.contains("missing leading ---"));
    }

    #[test]
    fn parse_missing_closing_marker_fails() {
        let dir = temp_dir("noclose");
        let path = dir.join("x.md");
        write(&path, "---\nname = \"x\"\nrole = \"coder\"\nbody only");
        let err = parse_persona_file(&path).unwrap_err();
        assert!(err.contains("no closing ---"));
    }

    #[test]
    fn parse_unknown_role_fails() {
        let dir = temp_dir("unknownrole");
        let path = dir.join("x.md");
        write(&path, "---\nname = \"x\"\nrole = \"wizard\"\n---\nbody");
        let err = parse_persona_file(&path).unwrap_err();
        assert!(err.contains("unknown role"), "got {err}");
    }

    #[test]
    fn parse_missing_name_fails() {
        let dir = temp_dir("noname");
        let path = dir.join("x.md");
        write(&path, "---\nrole = \"coder\"\n---\nbody");
        let err = parse_persona_file(&path).unwrap_err();
        assert!(err.contains("missing 'name'"), "got {err}");
    }

    #[test]
    fn parse_persona_without_examples_section() {
        let dir = temp_dir("noexamples");
        let path = dir.join("eva.md");
        write(
            &path,
            "---\nname = \"Eva\"\nrole = \"assistant\"\n---\n\nJust backstory, no examples.",
        );
        let p = parse_persona_file(&path).unwrap();
        assert_eq!(p.name, "Eva");
        assert_eq!(p.role, Role::Assistant);
        assert_eq!(p.examples.len(), 0);
        assert!(p.backstory.contains("Just backstory"));
    }

    #[test]
    fn load_personas_walks_directory() {
        let bundled = temp_dir("bundled");
        write(&bundled.join("codex7.md"), VALID_PERSONA);
        write(
            &bundled.join("sentinel9.md"),
            "---\nname = \"Sentinel-9\"\nrole = \"verifier\"\n---\n\nGuard duty.",
        );
        write(&bundled.join("not-a-persona.md"), "no frontmatter here");
        write(&bundled.join("ignored.txt"), "should be skipped");

        let map = load_personas(&bundled, None).unwrap();
        assert_eq!(
            map.len(),
            2,
            "two valid .md, one bad .md logged, .txt skipped"
        );
        assert!(map.contains_key("codex7"));
        assert!(map.contains_key("sentinel9"));
    }

    #[test]
    fn load_personas_user_dir_overrides_bundled() {
        let bundled = temp_dir("bundled-overlay");
        let user = temp_dir("user-overlay");
        write(&bundled.join("codex7.md"), VALID_PERSONA);
        write(
            &user.join("codex7.md"),
            "---\nname = \"CodeX-Override\"\nrole = \"coder\"\nvoice = \"my-voice\"\n---\n\nOverride body.",
        );
        let map = load_personas(&bundled, Some(&user)).unwrap();
        let cx = map.get("codex7").unwrap();
        assert_eq!(cx.name, "CodeX-Override");
        assert_eq!(cx.voice, "my-voice");
    }

    #[test]
    fn load_personas_missing_bundled_is_err() {
        let missing = std::env::temp_dir().join("claudette-forge-no-such-bundled-xyz");
        let _ = std::fs::remove_dir_all(&missing);
        let result = load_personas(&missing, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_role_accepts_case_and_separators() {
        assert_eq!(parse_role("Coder"), Some(Role::Coder));
        assert_eq!(parse_role("test-coder"), Some(Role::TestCoder));
        assert_eq!(parse_role("test_coder"), Some(Role::TestCoder));
        assert_eq!(parse_role("SurgicalCoder"), Some(Role::SurgicalCoder));
        assert_eq!(parse_role("unknown"), None);
    }

    // ─── Bundled personas smoke test ──────────────────────────────────
    //
    // The four .md files in `personas/` at the workspace root are the
    // shipping personas. If any of them stops parsing cleanly, the loader
    // would fail to start once forge-mode is wired in — exercise the real
    // files from the repo here.

    fn workspace_personas_dir() -> PathBuf {
        // CARGO_MANIFEST_DIR is `crates/claudette`; bundled personas live
        // alongside the crate at `crates/claudette/personas/` so they ship
        // inside the cargo-published tarball.
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("personas")
    }

    #[test]
    fn bundled_personas_all_parse() {
        let dir = workspace_personas_dir();
        let map = load_personas(&dir, None)
            .unwrap_or_else(|e| panic!("load_personas({}) failed: {e}", dir.display()));
        for key in ["codex7", "sentinel9", "cto", "eva"] {
            assert!(
                map.contains_key(key),
                "bundled persona '{key}' missing from map (keys: {:?})",
                map.keys().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn bundled_personas_have_expected_roles() {
        let dir = workspace_personas_dir();
        let map = load_personas(&dir, None).expect("load");
        assert_eq!(map["codex7"].role, Role::Coder);
        assert_eq!(map["sentinel9"].role, Role::Verifier);
        assert_eq!(map["cto"].role, Role::Cto);
        assert_eq!(map["eva"].role, Role::Assistant);
    }

    #[test]
    fn bundled_personas_are_loaded_not_placeholder() {
        let dir = workspace_personas_dir();
        let map = load_personas(&dir, None).expect("load");
        for (key, p) in &map {
            assert_ne!(
                p.status,
                PersonaStatus::Placeholder,
                "persona '{key}' still marked Placeholder"
            );
            assert!(
                !p.backstory.is_empty(),
                "persona '{key}' has empty backstory"
            );
            assert!(
                p.examples.len() >= 3,
                "persona '{key}' has only {} examples (expected ≥3)",
                p.examples.len()
            );
        }
    }
}
