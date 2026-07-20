//! Pure core of deep-research mode (`claudette --research`, wired in a
//! follow-up PR): walks a target repo into a deterministic review manifest,
//! plans 2-3-file batches, parses the reviewer's structured findings output,
//! and persists progress/findings JSON so an interrupted run resumes exactly
//! where it stopped.

use std::fmt::Write as _;

use crate::env_config;
use crate::run::build_research_runtime;
use crate::session::Session;

pub(crate) const MAX_BATCH_BYTES: u64 = 48 * 1024;
pub(crate) const MAX_FILE_BYTES: u64 = 256 * 1024;
pub(crate) const DEFAULT_BATCH_FILES: usize = 3;
pub(crate) const INCLUDE_EXTS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "rb", "cs", "cpp", "cc", "h", "hpp",
    "php", "md", "toml", "yml", "yaml",
];
pub(crate) const EXCLUDE_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
];

/// Paths excluded from every `--research` run by default. Conservative on
/// purpose: `docs/archive` is matched as a path prefix (not a bare segment) so
/// it only catches the conventional stale-docs tree, never a real
/// `src/archive/` source module. Operators add more via
/// `CLAUDETTE_RESEARCH_EXCLUDE`.
pub(crate) const DEFAULT_EXCLUDES: &[&str] = &["docs/archive"];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ManifestFile {
    pub rel_path: String,
    pub size: u64,
    pub mtime_secs: u64,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SkippedFile {
    pub rel_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Batch {
    pub id: usize,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Manifest {
    pub root: String,
    pub files: Vec<ManifestFile>,
    pub skipped: Vec<SkippedFile>,
    pub batches: Vec<Batch>,
    pub hash: String,
    /// Kept files whose content is dense with chat-template control tokens
    /// (`<|channel|>`, `<|end|>`, …). These reliably provoke content-less
    /// generation; the driver warns and points at `CLAUDETTE_RESEARCH_EXCLUDE`.
    /// Informational only — not part of the resume `hash`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flagged_control_tokens: Vec<String>,
}

/// Build a deterministic review manifest from `root` with no scope excludes.
/// Thin wrapper over [`build_manifest_with_excludes`]; the driver uses the
/// `_with_excludes` form so operator/default excludes apply. Test-only — the
/// production path always passes an exclude set.
#[cfg(test)]
pub(crate) fn build_manifest(
    root: &std::path::Path,
    max_batch_files: usize,
) -> Result<Manifest, String> {
    build_manifest_with_excludes(root, max_batch_files, &[])
}

/// Build a deterministic review manifest from `root`.
///
/// Walks the directory tree (respecting `.gitignore`), drops files matching
/// `excludes` (recorded as `skipped` with reason `"excluded"`), collects the
/// eligible remainder, plans them into size-aware batches, flags any file whose
/// content is dense with chat-template control tokens, and returns a
/// [`Manifest`].
#[allow(clippy::too_many_lines)] // card-prescribed algorithm, kept inline
pub(crate) fn build_manifest_with_excludes(
    root: &std::path::Path,
    max_batch_files: usize,
    excludes: &[String],
) -> Result<Manifest, String> {
    // Clamp max_batch_files to 1..=8.
    let max_batch_files = max_batch_files.clamp(1, 8);

    // Walk with ignore rules (repomap recipe). The builder methods take
    // `&mut self`, so the configuration can't be chained off the constructor.
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false);

    // Collect eligible files.
    let mut kept: Vec<ManifestFile> = Vec::new();
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let mut flagged: Vec<String> = Vec::new();

    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();

        // Check file name against EXCLUDE_FILES (case-sensitive).
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            if EXCLUDE_FILES.contains(&file_name) {
                continue;
            }
        } else {
            continue;
        }

        // Check extension (case-insensitive).
        let ext = path.extension().and_then(|e| e.to_str());
        let is_eligible =
            ext.is_some_and(|e| INCLUDE_EXTS.iter().any(|ie| ie.eq_ignore_ascii_case(e)));
        if !is_eligible {
            continue;
        }

        // Compute rel_path with forward slashes.
        let rel_path = path.strip_prefix(root).ok().map_or_else(
            || path.to_string_lossy().to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );

        // Scope excludes (defaults + CLAUDETTE_RESEARCH_EXCLUDE). Recorded, not
        // silently dropped, so coverage numbers stay honest.
        if path_is_excluded(&rel_path, excludes) {
            skipped.push(SkippedFile {
                rel_path,
                reason: "excluded".to_string(),
            });
            continue;
        }

        // Metadata.
        let meta = std::fs::metadata(path).ok();
        let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
        let mtime_secs = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        // Oversize check.
        if size > MAX_FILE_BYTES {
            skipped.push(SkippedFile {
                rel_path,
                reason: "oversize".to_string(),
            });
            continue;
        }

        // Read contents — not valid UTF-8 → unreadable.
        let Ok(content) = std::fs::read_to_string(path) else {
            skipped.push(SkippedFile {
                rel_path,
                reason: "unreadable".to_string(),
            });
            continue;
        };

        let lines = content.lines().count();

        // Files dense with chat-template control tokens reliably provoke
        // content-less generation; flag for the driver's warning.
        if content_has_control_tokens(&content) {
            flagged.push(rel_path.clone());
        }

        kept.push(ManifestFile {
            rel_path,
            size,
            mtime_secs,
            lines,
        });
    }

    // Sort by rel_path (byte-order).
    kept.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    skipped.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    // Batch planning — greedy fill.
    let mut batches: Vec<Batch> = Vec::new();
    let mut current_batch_files: Vec<String> = Vec::new();
    let mut current_batch_bytes: u64 = 0;
    let mut last_parent: Option<&str> = None;

    for file in &kept {
        let parent = file.rel_path.rsplit_once('/').map_or("", |(p, _)| p);

        // Check flush conditions BEFORE adding this file.
        let at_cap =
            !current_batch_files.is_empty() && current_batch_files.len() >= max_batch_files;
        let would_exceed_budget =
            !current_batch_files.is_empty() && (current_batch_bytes + file.size > MAX_BATCH_BYTES);
        let dir_changed = last_parent.is_some_and(|lp| lp != parent);

        // Flush if any condition triggers.
        if at_cap || would_exceed_budget || dir_changed {
            batches.push(Batch {
                id: batches.len() + 1,
                files: std::mem::take(&mut current_batch_files),
            });
            current_batch_bytes = 0;
        }

        // Add file to (possibly new) batch.
        current_batch_files.push(file.rel_path.clone());
        current_batch_bytes += file.size;
        last_parent = Some(parent);
    }

    // Flush remaining.
    if !current_batch_files.is_empty() {
        batches.push(Batch {
            id: batches.len() + 1,
            files: std::mem::take(&mut current_batch_files),
        });
    }

    // Zero files kept → error.
    if kept.is_empty() {
        return Err(format!("no reviewable files under {}", root.display()));
    }

    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = manifest_hash(&kept);

    flagged.sort();

    Ok(Manifest {
        root: canonical_root.to_string_lossy().to_string(),
        files: kept,
        skipped,
        batches,
        hash,
        flagged_control_tokens: flagged,
    })
}

/// FNV-1a 64-bit hash of a file manifest. No new dependency — inline impl.
pub(crate) fn manifest_hash(files: &[ManifestFile]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for f in files {
        let input = format!("{}\n{}\n{}\n", f.rel_path, f.size, f.mtime_secs);
        for byte in input.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    format!("{hash:016x}")
}

/// Parse a comma-separated exclude list into normalized entries: trimmed,
/// backslashes → `/`, trailing `/` stripped, empties dropped. No env access.
pub(crate) fn parse_exclude_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|e| e.trim().replace('\\', "/"))
        .map(|e| e.trim_end_matches('/').to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// The active exclude set for a run: [`DEFAULT_EXCLUDES`] plus whatever
/// `CLAUDETTE_RESEARCH_EXCLUDE` contributes. The only env reader here.
pub(crate) fn research_excludes() -> Vec<String> {
    let mut excludes: Vec<String> = DEFAULT_EXCLUDES.iter().map(|s| (*s).to_string()).collect();
    if let Ok(raw) = std::env::var("CLAUDETTE_RESEARCH_EXCLUDE") {
        excludes.extend(parse_exclude_list(&raw));
    }
    excludes
}

/// `true` when `rel_path` (forward-slash, repo-relative) is covered by any
/// exclude entry. An entry matches when the path equals it, sits under it as a
/// directory (`entry/` prefix), or — for a bare entry with no `/` — has a path
/// *segment* equal to it. So `harmony.rs` excludes the file at any depth and
/// `archive` excludes any `archive/` directory, but `archive` leaves
/// `archive.rs` (a distinct segment) alone.
pub(crate) fn path_is_excluded(rel_path: &str, excludes: &[String]) -> bool {
    excludes.iter().any(|e| {
        if rel_path == e || rel_path.starts_with(&format!("{e}/")) {
            return true;
        }
        !e.contains('/') && rel_path.split('/').any(|seg| seg == e)
    })
}

/// `true` when `content` holds three or more `<|` sequences — the shape of
/// Qwen/Harmony chat-template control tokens (`<|channel|>`, `<|end|>`, …).
/// Cheap and dependency-free; ordinary source never trips it, token-dense
/// files (e.g. `api/harmony.rs`) always do.
pub(crate) fn content_has_control_tokens(content: &str) -> bool {
    content.matches("<|").count() >= 3
}

// ── Finding types (step 6) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Severity {
    High,
    Medium,
    Low,
    Info,
}

impl Severity {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase().replace('_', "-");
        match normalized.as_str() {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Category {
    Bug,
    ErrorHandling,
    Security,
    DeadCode,
    DocsDrift,
    TestGap,
    Smell,
}

impl Category {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase().replace('_', "-");
        match normalized.as_str() {
            "bug" => Some(Self::Bug),
            "error-handling" => Some(Self::ErrorHandling),
            "security" => Some(Self::Security),
            "dead-code" => Some(Self::DeadCode),
            "docs-drift" => Some(Self::DocsDrift),
            "test-gap" => Some(Self::TestGap),
            "smell" => Some(Self::Smell),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
            Self::Info => "INFO",
        })
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bug => "bug",
            Self::ErrorHandling => "error-handling",
            Self::Security => "security",
            Self::DeadCode => "dead-code",
            Self::DocsDrift => "docs-drift",
            Self::TestGap => "test-gap",
            Self::Smell => "smell",
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Finding {
    pub file: String,
    pub line: Option<u32>,
    pub severity: Severity,
    pub category: Category,
    pub claim: String,
    pub evidence: String,
    pub failure: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ParsedBatch {
    pub verdict: String,
    pub findings: Vec<Finding>,
    pub dropped: Vec<String>,
}

/// Parse a model's batch output into structured findings.
///
/// The model is expected to emit `### BATCH VERDICT` and zero or more
/// `### FINDING` sections. See the card for the exact algorithm.
pub(crate) fn parse_batch_output(
    text: &str,
    root: &std::path::Path,
) -> Result<ParsedBatch, String> {
    let mut verdict = None;
    let mut findings: Vec<Finding> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    // Split into sections by header lines.
    let mut current_section: Option<(String, Vec<String>)> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        if lower == "### batch verdict" || lower == "### finding" {
            // Flush previous section.
            if let Some((header, body)) = current_section.take() {
                process_section(
                    &header,
                    &body,
                    root,
                    &mut verdict,
                    &mut findings,
                    &mut dropped,
                )?;
            }
            current_section = Some((trimmed.to_string(), Vec::new()));
        } else if let Some(ref mut sec) = current_section {
            sec.1.push(trimmed.to_string());
        }
    }

    // Flush last section.
    if let Some((header, body)) = current_section.take() {
        process_section(
            &header,
            &body,
            root,
            &mut verdict,
            &mut findings,
            &mut dropped,
        )?;
    }

    let verdict = verdict.ok_or_else(|| "missing BATCH VERDICT section".to_string())?;
    if verdict.trim().is_empty() {
        return Err("empty BATCH VERDICT body".to_string());
    }

    Ok(ParsedBatch {
        verdict,
        findings,
        dropped,
    })
}

#[allow(clippy::too_many_lines)] // card-prescribed algorithm, kept inline
fn process_section(
    header: &str,
    body_lines: &[String],
    root: &std::path::Path,
    verdict_out: &mut Option<String>,
    findings_out: &mut Vec<Finding>,
    dropped_out: &mut Vec<String>,
) -> Result<(), String> {
    if header.eq_ignore_ascii_case("### batch verdict") {
        // Card step 7.2: the verdict is the TRIMMED body of the section.
        let body = body_lines.join("\n").trim().to_string();
        *verdict_out = Some(body);
        return Ok(());
    }

    if !header.eq_ignore_ascii_case("### finding") {
        return Ok(()); // Ignore unknown sections.
    }

    // Parse key: value fields from the body. The free-text fields (claim /
    // evidence / failure) accept continuation lines: a non-key line appends
    // to whichever of them was opened last.
    fn append_field(buf: &mut String, text: &str) {
        if text.is_empty() {
            return;
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(text);
    }

    let mut file_val: Option<String> = None;
    let mut severity_val: Option<Severity> = None;
    let mut category_val: Option<Category> = None;
    let mut claim_val = String::new();
    let mut evidence_val = String::new();
    let mut failure_val = String::new();
    let mut open_field: Option<&'static str> = None;

    for line in body_lines {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        // Check if this is a key: value line.
        let lower_trimmed = trimmed.to_ascii_lowercase();
        let parsed_key = if lower_trimmed.starts_with("file:") {
            Some(("file", &trimmed["file:".len()..]))
        } else if lower_trimmed.starts_with("severity:") {
            Some(("severity", &trimmed["severity:".len()..]))
        } else if lower_trimmed.starts_with("category:") {
            Some(("category", &trimmed["category:".len()..]))
        } else if lower_trimmed.starts_with("claim:") {
            Some(("claim", &trimmed["claim:".len()..]))
        } else if lower_trimmed.starts_with("evidence:") {
            Some(("evidence", &trimmed["evidence:".len()..]))
        } else if lower_trimmed.starts_with("failure:") {
            Some(("failure", &trimmed["failure:".len()..]))
        } else {
            None
        };

        match parsed_key {
            Some(("file", val)) => {
                file_val = Some(val.trim().to_string());
                open_field = None;
            }
            Some(("severity", val)) => {
                severity_val =
                    Some(Severity::parse(val).ok_or_else(|| format!("bad severity: {val}"))?);
                open_field = None;
            }
            Some(("category", val)) => {
                category_val =
                    Some(Category::parse(val).ok_or_else(|| format!("bad category: {val}"))?);
                open_field = None;
            }
            Some(("claim", val)) => {
                append_field(&mut claim_val, val.trim());
                open_field = Some("claim");
            }
            Some(("evidence", val)) => {
                append_field(&mut evidence_val, val.trim());
                open_field = Some("evidence");
            }
            Some(("failure", val)) => {
                append_field(&mut failure_val, val.trim());
                open_field = Some("failure");
            }
            Some(_) => {}
            None => match open_field {
                Some("claim") => append_field(&mut claim_val, &trimmed),
                // Skip ``` / ```lang fence markers so the reviewer's fenced
                // evidence blocks don't leak the fences into stored evidence.
                Some("evidence") if !trimmed.starts_with("```") => {
                    append_field(&mut evidence_val, &trimmed);
                }
                Some("failure") => append_field(&mut failure_val, &trimmed),
                // Chatter before the first free-text key, or a fence line — ignored.
                _ => {}
            },
        }
    }

    // All six keys are required and non-empty (card step 7).
    let Some(file) = file_val.filter(|f| !f.is_empty()) else {
        return Err("missing or empty key: file".to_string());
    };
    let Some(severity) = severity_val else {
        return Err("missing key: severity".to_string());
    };
    let Some(category) = category_val else {
        return Err("missing key: category".to_string());
    };
    if claim_val.is_empty() {
        return Err("missing or empty key: claim".to_string());
    }
    if evidence_val.is_empty() {
        return Err("missing or empty key: evidence".to_string());
    }
    if failure_val.is_empty() {
        return Err("missing or empty key: failure".to_string());
    }
    let claim = claim_val;
    let failure = failure_val;

    // Normalize file path.
    let mut norm_file = file.replace('\\', "/");
    if norm_file.starts_with("./") {
        norm_file = norm_file[2..].to_string();
    }

    // Strip a trailing line-spec into the line number. Accepts `:N`, `:N-M`
    // (range → take N), and `:N:C` (line:col → N): after the FIRST colon the
    // leading run of digits is the line, and the rest (`-M`, `:C`) is dropped.
    // Use the first colon, not the last, so `path:6:3` doesn't leave a stray
    // `path:6` that the unsafe-path guard below would then reject.
    let mut line: Option<u32> = None;
    if let Some(colon_pos) = norm_file.find(':') {
        let digits: String = norm_file[colon_pos + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() {
            line = digits.parse::<u32>().ok();
            norm_file = norm_file[..colon_pos].to_string();
        }
    }

    // Drop check: path starts with /, contains :, has .. segment, or doesn't
    // exist as a file under root.
    if norm_file.starts_with('/') || norm_file.contains(':') || norm_file.contains("..") {
        dropped_out.push(format!("dropped '{norm_file}': unsafe path"));
        return Ok(());
    }

    let check_path = root.join(&norm_file);
    if !check_path.is_file() {
        dropped_out.push(format!("dropped '{norm_file}': file not found under root"));
        return Ok(());
    }

    findings_out.push(Finding {
        file: norm_file,
        line,
        severity,
        category,
        claim,
        evidence: evidence_val,
        failure,
    });

    Ok(())
}

// ── Progress + Findings stores (step 8) ───────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    Batches,
    Verify,
    Synthesize,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BatchState {
    Pending,
    Done,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BatchStatus {
    pub id: usize,
    pub state: BatchState,
    pub attempts: u32,
    pub findings: usize,
    pub wall_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Progress {
    pub manifest_hash: String,
    pub started_unix: u64,
    pub phase: Phase,
    pub batches: Vec<BatchStatus>,
}

impl Progress {
    /// Create a new progress store with all batches in `Pending` state.
    pub(crate) fn new(manifest: &Manifest, started_unix: u64) -> Self {
        let batches = manifest
            .batches
            .iter()
            .map(|b| BatchStatus {
                id: b.id,
                state: BatchState::Pending,
                attempts: 0,
                findings: 0,
                wall_secs: 0,
                skip_reason: None,
            })
            .collect();
        Self {
            manifest_hash: manifest.hash.clone(),
            started_unix,
            phase: Phase::Batches,
            batches,
        }
    }

    /// Return the id of the first pending batch, or `None` if all are done.
    pub(crate) fn next_pending(&self) -> Option<usize> {
        self.batches
            .iter()
            .find(|b| b.state == BatchState::Pending)
            .map(|b| b.id)
    }

    /// Load a progress store from disk. Returns `Ok(None)` if the file
    /// doesn't exist; malformed JSON → `Err`.
    pub(crate) fn load(path: &std::path::Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read progress file: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("malformed progress JSON: {e}"))
    }

    /// Save the progress store to disk as pretty-printed JSON.
    pub(crate) fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize progress: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("failed to write progress file: {e}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Confirmed,
    Retracted,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredFinding {
    pub id: usize,
    pub batch: usize,
    #[serde(flatten)]
    pub finding: Finding,
    pub verdict: Option<Verdict>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FindingsStore {
    pub findings: Vec<StoredFinding>,
}

impl FindingsStore {
    /// Create an empty store.
    pub(crate) fn new() -> Self {
        Self {
            findings: Vec::new(),
        }
    }

    /// Load a findings store from disk. Returns `Ok(None)` if the file
    /// doesn't exist; malformed JSON → `Err`.
    pub(crate) fn load(path: &std::path::Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read findings file: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("malformed findings JSON: {e}"))
    }

    /// Save the findings store to disk as pretty-printed JSON.
    pub(crate) fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize findings: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("failed to write findings file: {e}"))
    }

    /// Append findings from a batch. IDs continue 1-based across the whole store.
    pub(crate) fn append_batch(&mut self, batch_id: usize, findings: &[Finding]) {
        let next_id = self.findings.len() + 1;
        for (i, f) in findings.iter().enumerate() {
            self.findings.push(StoredFinding {
                id: next_id + i,
                batch: batch_id,
                finding: f.clone(),
                verdict: None,
            });
        }
    }
}

/// Build the terse per-batch user prompt (SPEC §6). `files` is the batch's
/// (rel_path, line_count) pairs; `focus` is the optional CLI focus hint
/// (empty string when none).
fn batch_prompt(n: usize, total: usize, files: &[(String, usize)], focus: &str) -> String {
    let mut out = format!("Batch {n}/{total}. Files:\n");
    for (path, lines) in files {
        let _ = writeln!(out, "{path} ({lines} lines)");
    }
    if !focus.is_empty() {
        let _ = writeln!(out, "{focus}");
    }
    out.push_str(
        "Review per your briefing, then output:\n\
         \n\
         ### BATCH VERDICT\n\
         <2-4 sentences: what these files do, overall health, anything odd>\n\
         \n\
         Then zero or more:\n\
         \n\
         ### FINDING\n\
         file: <repo-relative path>:<line>\n\
         severity: HIGH|MEDIUM|LOW|INFO\n\
         category: bug|error-handling|security|dead-code|docs-drift|test-gap|smell\n\
         claim: <one sentence>\n\
         evidence: <quoted code, 1-3 lines>\n\
         failure: <concrete scenario: inputs/state -> wrong outcome>",
    );
    out
}

/// The reviewer briefing system prompt (SPEC §6). `{root}` is substituted at build time.
const RESEARCH_BRIEFING: &str = r"You are claudette in deep-research mode: a careful, skeptical code reviewer on
a strictly read-only audit of the repository at {root}. You cannot modify
anything; write tools are disabled and will be refused.

Task per batch: read each assigned file COMPLETELY (read_file; large files in
chunks), then report findings. Use repo_map/grep_search to check how the code
is used elsewhere before judging it.

Review lenses, in priority order: (1) correctness bugs, (2) error-handling and
edge cases, (3) security, (4) dead or unreachable code, (5) comment/doc drift
vs actual behavior, (6) missing or weak tests.

Rules:
- Report at most 5 findings per batch — only what you would defend to a
  skeptical reviewer. A clean file is a valid result; say so and move on.
- NEVER report a finding without a concrete failure scenario. No scenario,
  no finding.
- Cite exact file:line and quote the relevant code.
- End with the required output format. Output nothing after it.";

// ── Driver helpers (step 6) ────────────────────────────────────────────────

/// Resolve the target root for a research run.
/// `CLAUDETTE_WORKSPACE` first (first entry if platform-separated list; must be absolute),
/// else `git rev-parse --show-toplevel`. Refuse with a clear error if neither resolves.
fn resolve_target_root() -> Result<std::path::PathBuf, String> {
    // Try CLAUDETTE_WORKSPACE first.
    if let Ok(workspace) = std::env::var("CLAUDETTE_WORKSPACE") {
        for entry in workspace.split(';') {
            let path = entry.trim();
            if !path.is_empty() && std::path::Path::new(path).is_absolute() {
                return Ok(std::path::PathBuf::from(path));
            }
        }
    }

    // Fallback: git toplevel.
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(std::path::PathBuf::from(path));
            }
        }
        _ => {}
    }

    Err("neither CLAUDETTE_WORKSPACE (absolute) nor git toplevel resolved".into())
}

/// Resolve the output directory for research results.
/// `CLAUDETTE_RESEARCH_DIR` used as-is if set; else
/// `~/.claudette/research/<repo-dirname>-<YYYY-MM-DD>/`. Create it.
fn resolve_output_dir(root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    // Override: used as-is, but it must never sit inside the target tree —
    // the read-only promise covers the whole repo, so writing findings into
    // it would break that guarantee (SPEC §6b). Compare canonical paths when
    // both resolve; fall back to the lexical path for a not-yet-created dir.
    if let Ok(dir) = std::env::var("CLAUDETTE_RESEARCH_DIR") {
        let dir = std::path::PathBuf::from(dir);
        let root_c = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let dir_c = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if dir_c.starts_with(&root_c) {
            return Err(format!(
                "CLAUDETTE_RESEARCH_DIR ({}) is inside the target tree {}; \
                 choose a path outside it",
                dir.display(),
                root.display()
            ));
        }
        return Ok(dir);
    }

    // Default: ~/.claudette/research/<repo-dirname>-<YYYY-MM-DD>/
    let repo_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let date = current_date_str();

    let home = env_config::home_dir();
    Ok(home
        .join(".claudette")
        .join("research")
        .join(format!("{repo_name}-{date}")))
}

/// Set `CLAUDETTE_OFFLINE=1` unless already set.
fn force_offline_for_run() {
    if std::env::var(crate::egress::OFFLINE_ENV).is_err() {
        std::env::set_var(crate::egress::OFFLINE_ENV, "1");
    }
}

/// Point `CLAUDETTE_WORKSPACE` at the target root unless already set, so the
/// per-batch read-only runtimes may read the reviewed repo when it lives
/// outside `$HOME` (the file sandbox re-reads the env on every request).
/// When the var is already set, `resolve_target_root` derived the root from
/// it, so there is nothing to do.
fn force_workspace_for_run(root: &std::path::Path) {
    if std::env::var("CLAUDETTE_WORKSPACE").is_err() {
        std::env::set_var("CLAUDETTE_WORKSPACE", root);
    }
}

/// Read `CLAUDETTE_RESEARCH_BATCH_FILES`, parse, clamp 1..=8, default DEFAULT_BATCH_FILES.
fn batch_files_from_env() -> usize {
    std::env::var("CLAUDETTE_RESEARCH_BATCH_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(crate::run::research::DEFAULT_BATCH_FILES)
        .clamp(1, 8)
}

/// Read `CLAUDETTE_RESEARCH_MAX_BATCHES`, Some(n) if positive integer, else None.
fn max_batches_from_env() -> Option<usize> {
    std::env::var("CLAUDETTE_RESEARCH_MAX_BATCHES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
}

/// Health-probe system prompt (driver-side liveness check between attempts).
const PROBE_SYSTEM: &str =
    "You are a health probe. Reply with exactly: OK. No other text, no tool calls.";

/// Health-probe user prompt.
const PROBE_PROMPT: &str = "Reply with exactly: OK";

/// Probe rounds per recovery stage before giving up on the backend.
const PROBE_ROUNDS: usize = 5;

/// Seconds to sleep before each recovery probe.
const PROBE_SLEEP_SECS: u64 = 60;

/// `CLAUDETTE_RESEARCH_RETRY_SKIPPED=1` re-queues previously skipped batches
/// on resume.
fn retry_skipped_enabled() -> bool {
    std::env::var("CLAUDETTE_RESEARCH_RETRY_SKIPPED").is_ok_and(|v| v == "1")
}

/// Optional driver-side recovery command (`CLAUDETTE_RESEARCH_RECOVER_CMD`).
fn recover_cmd_from_env() -> Option<String> {
    std::env::var("CLAUDETTE_RESEARCH_RECOVER_CMD")
        .ok()
        .filter(|c| !c.trim().is_empty())
}

/// `true` when a probe response counts as healthy (any non-whitespace content).
fn probe_healthy(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Flip every `Skipped` batch back to `Pending`; returns how many were flipped.
fn flip_skipped_to_pending(progress: &mut Progress) -> usize {
    let mut flipped = 0usize;
    for b in &mut progress.batches {
        if b.state == BatchState::Skipped {
            b.state = BatchState::Pending;
            b.attempts = 0;
            b.skip_reason = None;
            flipped += 1;
        }
    }
    flipped
}

/// Resume decision per SPEC §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeAction {
    Fresh,
    ResumeAt(usize),
    RefuseChanged,
    RefuseDone,
}

/// Compute the resume action given existing progress and a manifest hash.
fn resume_action(existing: Option<&Progress>, manifest_hash: &str) -> ResumeAction {
    let Some(prog) = existing else {
        return ResumeAction::Fresh;
    };
    if prog.manifest_hash != manifest_hash {
        return ResumeAction::RefuseChanged;
    }
    if prog.phase == Phase::Done {
        return ResumeAction::RefuseDone;
    }
    match prog.next_pending() {
        Some(id) => ResumeAction::ResumeAt(id),
        None => ResumeAction::Fresh, // all done but phase != Done → treat as fresh batches
    }
}

/// Format the FINDINGS.md run header (SPEC §10).
fn findings_md_run_header(
    root: &std::path::Path,
    output_dir: &std::path::Path,
    manifest: &Manifest,
) -> String {
    let total_files = manifest.files.len();
    let skipped_oversize = manifest
        .skipped
        .iter()
        .filter(|s| s.reason == "oversize")
        .count();
    let skipped_unreadable = manifest
        .skipped
        .iter()
        .filter(|s| s.reason == "unreadable")
        .count();
    let skipped_excluded = manifest
        .skipped
        .iter()
        .filter(|s| s.reason == "excluded")
        .count();

    format!(
        "# Deep Research Findings\n\
         \n\
         **Target:** `{}`  \n\
         **Output dir:** `{}`  \n\
         **Date:** {}  \n\
         **Files reviewed:** {total_files}  \n\
         **Skipped (oversize):** {skipped_oversize}  \n\
         **Skipped (unreadable):** {skipped_unreadable}  \n\
         **Skipped (excluded):** {skipped_excluded}\n",
        root.display(),
        output_dir.display(),
        current_date_str()
    )
}

fn current_date_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Format a batch block for FINDINGS.md.
fn findings_md_batch_block(
    batch_id: usize,
    files: &[(String, usize)],
    parsed: &ParsedBatch,
) -> String {
    let mut out = format!("\n## Batch {}\n\n", batch_id);
    for (path, lines) in files {
        let _ = writeln!(out, "  - `{path}` ({lines} lines)");
    }
    out.push_str("### Verdict\n");
    out.push_str(&parsed.verdict);
    if !parsed.findings.is_empty() {
        out.push_str("\n\n### Findings\n");
        for (i, f) in parsed.findings.iter().enumerate() {
            let file_line = match f.line {
                Some(l) => format!("{}:{}", f.file, l),
                None => f.file.clone(),
            };
            let _ = write!(
                out,
                "\n#### Finding {}\n\n- **File:** `{}`\n- **Severity:** {}\n- **Category:** {}\n- **Claim:** {}\n- **Evidence:**\n  ```\n{}\n```\n- **Failure:** {}\n",
                i + 1,
                file_line,
                f.severity,
                f.category,
                f.claim,
                f.evidence,
                f.failure,
            );
        }
    } else {
        out.push_str("\n\nNo findings.\n");
    }
    out
}

/// Format a skipped-batch note for FINDINGS.md.
fn findings_md_skipped_note(batch_id: usize, reason: &str) -> String {
    format!(
        "\n## Batch {} — SKIPPED\n\nBatch skipped after retries: {}.\n",
        batch_id, reason
    )
}

/// Classify a raw model response into an attempt outcome.
#[derive(Debug)]
pub(crate) enum AttemptOutcome {
    Parsed(ParsedBatch),
    Empty,
    ParseError(String),
}

fn classify_attempt(text: &str, root: &std::path::Path) -> AttemptOutcome {
    if text.trim().is_empty() {
        return AttemptOutcome::Empty;
    }
    match parse_batch_output(text, root) {
        Ok(parsed) => AttemptOutcome::Parsed(parsed),
        Err(e) => AttemptOutcome::ParseError(e),
    }
}

/// One cheap generation probe. `true` = backend produced visible content.
fn probe_backend() -> bool {
    let session = Session::new();
    let mut runtime = build_research_runtime(session, vec![PROBE_SYSTEM.to_string()]);
    match crate::brain_selector::run_turn_with_fallback(&mut runtime, PROBE_PROMPT, &mut None) {
        Ok(summary) => probe_healthy(&crate::run::extract_assistant_text(&summary)),
        Err(_) => false,
    }
}

/// Wait out a backend sick-episode (consecutive content-less turns).
///
/// Stage 1: up to `PROBE_ROUNDS` probes, `PROBE_SLEEP_SECS` apart. If all fail
/// and `CLAUDETTE_RESEARCH_RECOVER_CMD` is set and unused this run, run it
/// driver-side (the model never sees it) and probe one more stage.
/// `true` = backend is generating again.
fn recover_backend(recover_cmd_used: &mut bool) -> bool {
    for stage in 0..2u8 {
        for round in 1..=PROBE_ROUNDS {
            eprintln!(
                "  backend produced no content — recovery probe {round}/{PROBE_ROUNDS} in {PROBE_SLEEP_SECS}s"
            );
            std::thread::sleep(std::time::Duration::from_secs(PROBE_SLEEP_SECS));
            if probe_backend() {
                eprintln!("  backend recovered");
                return true;
            }
        }
        if stage == 1 {
            break;
        }
        let Some(cmd) = recover_cmd_from_env() else {
            return false;
        };
        if *recover_cmd_used {
            return false;
        }
        *recover_cmd_used = true;
        eprintln!("  running CLAUDETTE_RESEARCH_RECOVER_CMD: {cmd}");
        let launched = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", &cmd])
                .status()
        } else {
            std::process::Command::new("sh").args(["-c", &cmd]).status()
        };
        if let Err(e) = launched {
            eprintln!("  recover command failed to launch: {e}");
            return false;
        }
    }
    false
}

/// The main driver: `claudette --research` entry point.
#[allow(clippy::too_many_lines)]
pub fn run_deep_research(focus: &str) -> anyhow::Result<()> {
    // 1. Force offline, resolve root, point the workspace boundary at it.
    force_offline_for_run();
    let root = resolve_target_root().map_err(|e| anyhow::anyhow!("{}", e))?;
    force_workspace_for_run(&root);
    let output_dir = resolve_output_dir(&root).map_err(|e| anyhow::anyhow!("{}", e))?;

    // Ensure output dir is not inside the target tree.
    if output_dir.starts_with(&root) {
        return Err(anyhow::anyhow!(
            "CLAUDETTE_RESEARCH_DIR resolves inside the target tree — refuse to avoid write surface"
        ));
    }
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| anyhow::anyhow!("failed to create output dir: {e}"))?;

    // 2. Build or load manifest.json.
    let manifest =
        build_manifest_with_excludes(&root, batch_files_from_env(), &research_excludes())
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    if !manifest.flagged_control_tokens.is_empty() {
        eprintln!(
            "warning: {} file(s) are dense with chat-template control tokens and \
             often provoke content-less batches (skips/retries):",
            manifest.flagged_control_tokens.len()
        );
        for path in &manifest.flagged_control_tokens {
            eprintln!("  {path}");
        }
        eprintln!(
            "  exclude them with CLAUDETTE_RESEARCH_EXCLUDE if the flake cost isn't worth it."
        );
    }
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| anyhow::anyhow!("failed to serialize manifest: {e}"))?;
    std::fs::write(output_dir.join("manifest.json"), &manifest_json)
        .map_err(|e| anyhow::anyhow!("failed to write manifest: {e}"))?;

    // 3. Load or init progress.json.
    let progress_path = output_dir.join("progress.json");
    let mut existing_progress = Progress::load(&progress_path).map_err(|e| anyhow::anyhow!(e))?;
    if let Some(prog) = existing_progress.as_mut() {
        if retry_skipped_enabled() && prog.phase == Phase::Batches {
            let flipped = flip_skipped_to_pending(prog);
            if flipped > 0 {
                eprintln!("retry-skipped: re-queued {flipped} previously skipped batch(es)");
            }
        }
    }
    let resume = resume_action(existing_progress.as_ref(), &manifest.hash);

    match resume {
        ResumeAction::RefuseChanged => {
            return Err(anyhow::anyhow!(
                "Manifest hash changed since last run. Delete {} or set CLAUDETTE_RESEARCH_DIR to start over.",
                output_dir.display()
            ));
        }
        ResumeAction::RefuseDone => {
            return Err(anyhow::anyhow!(
                "Previous run completed (phase=done). Check REPORT.md in {}. Set CLAUDETTE_RESEARCH_DIR for a fresh run.",
                output_dir.display()
            ));
        }
        _ => {}
    }

    let mut progress = match resume {
        ResumeAction::Fresh => {
            let started = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            Progress::new(&manifest, started)
        }
        ResumeAction::ResumeAt(id) => {
            // ResumeAt is only produced when existing progress was present.
            let prog = existing_progress.expect("ResumeAt implies existing progress");
            eprintln!("resuming at batch {}", id);
            prog
        }
        _ => unreachable!(),
    };

    // Write findings header for fresh runs.
    if matches!(resume, ResumeAction::Fresh) {
        let header = findings_md_run_header(&root, &output_dir, &manifest);
        std::fs::write(output_dir.join("FINDINGS.md"), header)
            .map_err(|e| anyhow::anyhow!("failed to write FINDINGS.md: {e}"))?;
    }

    // Init empty findings store for fresh runs.
    let mut findings_store = match resume {
        ResumeAction::Fresh => FindingsStore::new(),
        _ => FindingsStore::load(&output_dir.join("findings.json"))
            .map_err(|e| anyhow::anyhow!(e))?
            .unwrap_or_else(FindingsStore::new),
    };

    // 4. Print header (SPEC §10).
    eprintln!("Deep research: {}", root.display());
    eprintln!("Output dir: {}", output_dir.display());
    eprintln!(
        "Files: {} | Batches: {}",
        manifest.files.len(),
        manifest.batches.len()
    );
    eprintln!("Offline mode enforced");
    eprintln!("Read-only permission tier (write/exec/network denied)");

    // 5. Batch loop.
    let max_batches = max_batches_from_env();
    let mut batches_done = 0usize;
    let mut batches_skipped = 0usize;
    let mut recover_cmd_used = false;
    let mut total_findings = 0usize;
    let mut high_count = 0usize;

    for batch in &manifest.batches {
        let is_pending = progress
            .batches
            .iter()
            .any(|b| b.id == batch.id && b.state == BatchState::Pending);
        if !is_pending {
            continue; // done or skipped in a previous run
        }
        if let Some(max) = max_batches {
            if batches_done >= max {
                break; // stopped by max_batches knob
            }
        }

        let files: Vec<(String, usize)> = batch
            .files
            .iter()
            .filter_map(|rel_path| {
                manifest
                    .files
                    .iter()
                    .find(|f| f.rel_path == *rel_path)
                    .map(|mf| (mf.rel_path.clone(), mf.lines))
            })
            .collect();

        let system_prompt = vec![RESEARCH_BRIEFING.replace("{root}", &root.display().to_string())];

        // Attempt ladder: parse errors burn a 2-attempt budget; content-less
        // turns get one immediate retry, then a probe-gated recovery wait, then
        // one final attempt. Skips are batch-bound only; a dead backend halts
        // the run checkpointed instead of punching coverage holes.
        let mut attempt_count: u32 = 0;
        let mut empty_streak: u32 = 0;
        let mut recovered_once = false;
        let mut parsed_batch: Option<ParsedBatch> = None;
        let mut skip_reason: Option<String> = None;
        let mut batch_status = BatchStatus {
            id: batch.id,
            state: BatchState::Pending,
            attempts: 0,
            findings: 0,
            wall_secs: 0,
            skip_reason: None,
        };

        loop {
            attempt_count += 1;
            batch_status.attempts = attempt_count;
            let start = std::time::SystemTime::now();

            let session = Session::new();
            let mut runtime = build_research_runtime(session, system_prompt.clone());
            let prompt = if attempt_count > 1 && empty_streak == 0 {
                format!(
                    "{}\n\nYour previous output did not match the required format. Re-output now, format only.",
                    batch_prompt(batch.id, manifest.batches.len(), &files, focus)
                )
            } else {
                batch_prompt(batch.id, manifest.batches.len(), &files, focus)
            };

            let text = match crate::brain_selector::run_turn_with_fallback(
                &mut runtime,
                &prompt,
                &mut None,
            ) {
                Ok(summary) => crate::run::extract_assistant_text(&summary),
                Err(e) => {
                    eprintln!("  batch {}: turn error: {e}", batch.id);
                    String::new()
                }
            };
            batch_status.wall_secs += start.elapsed().map_or(0, |d| d.as_secs());

            match classify_attempt(&text, &root) {
                AttemptOutcome::Parsed(pb) => {
                    parsed_batch = Some(pb);
                    break;
                }
                AttemptOutcome::Empty => {
                    empty_streak += 1;
                    if empty_streak == 1 {
                        continue;
                    }
                    if recovered_once {
                        skip_reason = Some(
                            "no content from a verified-healthy backend (batch-bound)".to_string(),
                        );
                        break;
                    }
                    if recover_backend(&mut recover_cmd_used) {
                        recovered_once = true;
                        empty_streak = 1;
                        continue;
                    }
                    progress.save(&progress_path).ok();
                    return Err(anyhow::anyhow!(
                        "backend stopped producing content and did not recover; \
                         run checkpointed — re-invoke to resume at batch {}",
                        batch.id
                    ));
                }
                AttemptOutcome::ParseError(e) => {
                    empty_streak = 0;
                    if attempt_count >= 2 {
                        skip_reason = Some(format!("findings did not parse: {e}"));
                        break;
                    }
                    eprintln!(
                        "  batch {}: findings did not parse ({e}) — retrying",
                        batch.id
                    );
                }
            }
        }

        if let Some(reason) = skip_reason {
            batch_status.state = BatchState::Skipped;
            batch_status.skip_reason = Some(reason.clone());
            batches_skipped += 1;
            eprintln!("  batch {}: SKIPPED — {reason}", batch.id);
            std::fs::write(
                output_dir.join("FINDINGS.md"),
                format!(
                    "{}\n{}",
                    std::fs::read_to_string(output_dir.join("FINDINGS.md")).unwrap_or_default(),
                    findings_md_skipped_note(batch.id, &reason)
                ),
            )
            .ok();
        }

        if let Some(pb) = parsed_batch {
            // Success: append to FINDINGS.md and findings.json.
            std::fs::write(
                output_dir.join("FINDINGS.md"),
                format!(
                    "{}{}",
                    std::fs::read_to_string(output_dir.join("FINDINGS.md")).unwrap_or_default(),
                    findings_md_batch_block(batch.id, &files, &pb)
                ),
            )
            .ok();

            findings_store.append_batch(batch.id, &pb.findings);
            findings_store.save(&output_dir.join("findings.json")).ok();

            batch_status.state = BatchState::Done;
            batch_status.findings = pb.findings.len();
            total_findings += pb.findings.len();
            high_count += pb
                .findings
                .iter()
                .filter(|f| f.severity == Severity::High)
                .count();
        }

        // Capture the fields used by the stderr line before `batch_status`
        // is moved into the progress vector below.
        let batch_findings = batch_status.findings;
        let batch_wall = batch_status.wall_secs;

        // Update progress.
        if let Some(status) = progress.batches.iter_mut().find(|b| b.id == batch.id) {
            *status = batch_status;
        }
        progress.save(&progress_path).ok();

        batches_done += 1;

        // Per-batch stderr line (SPEC §10).
        eprintln!(
            "[{}/{}] {} — {} findings ({} HIGH) — {}s",
            batch.id,
            manifest.batches.len(),
            root.display(),
            batch_findings,
            high_count,
            batch_wall,
        );
    }

    // 6. After the batch loop: if every batch is resolved and this run was not
    // capped by CLAUDETTE_RESEARCH_MAX_BATCHES, advance through the verify and
    // synthesize phases (SPEC §9). Each phase is gated on progress.phase and
    // saved on completion, so a resumed run picks up exactly where it stopped.
    let all_done = progress
        .batches
        .iter()
        .all(|b| b.state == BatchState::Done || b.state == BatchState::Skipped);
    if all_done && max_batches.is_none() {
        if progress.phase == Phase::Batches {
            progress.phase = Phase::Verify;
            progress.save(&progress_path).ok();
        }
        if progress.phase == Phase::Verify {
            run_verify_pass(&root, &output_dir, &mut findings_store);
            progress.phase = Phase::Synthesize;
            progress.save(&progress_path).ok();
        }
        if progress.phase == Phase::Synthesize {
            run_synthesize_pass(
                &root,
                &output_dir,
                &manifest,
                &findings_store,
                batches_done,
                batches_skipped,
            )?;
            progress.phase = Phase::Done;
            progress.save(&progress_path).ok();
        }
    } else {
        progress.save(&progress_path).ok();
    }

    // 7. Footer (SPEC §10).
    eprintln!(
        "\nDeep research complete: {} batches, {} skipped, {} findings ({} HIGH)",
        batches_done, batches_skipped, total_findings, high_count
    );
    eprintln!("Output dir: {}", output_dir.display());

    Ok(())
}

/// System prompt for one verify-pass conversation (SPEC §9). `{root}` is
/// substituted by the driver at build time.
const RESEARCH_VERIFY_BRIEFING: &str = "\
You are claudette verifying a single code-review finding on a strictly \
read-only audit of the repository at {root}. Write tools are disabled.

Re-read the cited file (read_file; use grep_search or repo_map to check how \
the code is actually used) and decide whether the finding's failure scenario \
truly holds. You are rewarded for RETRACTING weak, speculative, or incorrect \
findings — not for defending them. Confirm only what you would stake your \
reputation on.

Output EXACTLY two lines, nothing before or after:
VERDICT: CONFIRMED or RETRACTED
reason: <one sentence>";

/// Parse a verify-pass response into a verdict. Looks for a line beginning
/// `VERDICT:` (case-insensitive) carrying CONFIRMED or RETRACTED. Returns
/// None if neither token is present (driver retries, then marks Unverified).
fn parse_verdict(text: &str) -> Option<Verdict> {
    for line in text.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("verdict:") {
            if rest.contains("confirmed") {
                return Some(Verdict::Confirmed);
            }
            if rest.contains("retracted") {
                return Some(Verdict::Retracted);
            }
        }
    }
    None
}

/// System prompt for the single synthesize conversation (SPEC §9).
const RESEARCH_SYNTH_BRIEFING: &str = "\
You are claudette writing the final report for a completed, strictly \
read-only review of the repository at {root}. You are given a table of every \
finding with its verification verdict. Do not read files; work only from the \
table.

Write a concise, triage-ready report in Markdown:
- Executive summary: 3-5 sentences on overall health, coverage, and the \
headline risks.
- Top findings, ranked (CONFIRMED first, then by severity). One bullet each: \
`file:line` — severity — one-sentence claim.
- Recurring themes: patterns across findings, if any.
- Suggested missions: 3-7 card-sized work items, each as \
`<title> — why it matters — files touched`.

Output only the report body. Do not reprint the raw table.";

/// A finding needs verification iff it is HIGH or MEDIUM and has no verdict
/// yet (LOW/INFO skip verification; a set verdict means a prior run did it).
fn needs_verify(f: &StoredFinding) -> bool {
    f.verdict.is_none() && matches!(f.finding.severity, Severity::High | Severity::Medium)
}

/// One-word label for a stored verdict; `—` when the finding was never sent
/// to verification (LOW/INFO).
fn verdict_label(v: Option<Verdict>) -> &'static str {
    match v {
        Some(Verdict::Confirmed) => "confirmed",
        Some(Verdict::Retracted) => "retracted",
        Some(Verdict::Unverified) => "unverified",
        None => "—",
    }
}

/// The user prompt for verifying one finding (SPEC §9).
fn verify_prompt(f: &Finding) -> String {
    let file_line = match f.line {
        Some(l) => format!("{}:{}", f.file, l),
        None => f.file.clone(),
    };
    format!(
        "Finding to verify:\n\n\
         file: {}\nseverity: {}\ncategory: {}\nclaim: {}\nevidence: {}\nfailure: {}\n\n\
         Re-read the cited file and decide. Output the two-line verdict only.",
        file_line, f.severity, f.category, f.claim, f.evidence, f.failure,
    )
}

/// Compact pipe table of all findings for the synthesis prompt (SPEC §9):
/// id, batch, file:line, severity, category, verdict, claim. No prose.
fn findings_table(store: &FindingsStore) -> String {
    let mut out = String::from(
        "| id | batch | file:line | severity | category | verdict | claim |\n\
         |---|---|---|---|---|---|---|\n",
    );
    for sf in &store.findings {
        let file_line = match sf.finding.line {
            Some(l) => format!("{}:{}", sf.finding.file, l),
            None => sf.finding.file.clone(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            sf.id,
            sf.batch,
            file_line,
            sf.finding.severity,
            sf.finding.category,
            verdict_label(sf.verdict),
            sf.finding.claim.replace('|', "\\|"),
        );
    }
    out
}

/// Driver-generated metadata header for REPORT.md (SPEC §9). Counts come
/// from the store; coverage from the manifest.
fn report_metadata_header(
    root: &std::path::Path,
    model: &str,
    manifest: &Manifest,
    store: &FindingsStore,
    batches_done: usize,
    batches_skipped: usize,
) -> String {
    let total = store.findings.len();
    let high = store
        .findings
        .iter()
        .filter(|f| f.finding.severity == Severity::High)
        .count();
    let confirmed = store
        .findings
        .iter()
        .filter(|f| f.verdict == Some(Verdict::Confirmed))
        .count();
    let retracted = store
        .findings
        .iter()
        .filter(|f| f.verdict == Some(Verdict::Retracted))
        .count();
    let unverified = store
        .findings
        .iter()
        .filter(|f| f.verdict == Some(Verdict::Unverified))
        .count();
    let oversize = manifest
        .skipped
        .iter()
        .filter(|s| s.reason == "oversize")
        .count();
    format!(
        "# Deep Research Report\n\
         \n\
         **Target:** `{}`  \n\
         **Date:** {}  \n\
         **Model:** {model}  \n\
         **Batches:** {batches_done} done, {batches_skipped} skipped  \n\
         **Findings:** {total} ({high} HIGH) — {confirmed} confirmed, {retracted} retracted, {unverified} unverified  \n\
         **Coverage:** {} files reviewed, {oversize} skipped-oversize\n\
         \n---\n",
        root.display(),
        current_date_str(),
        manifest.files.len(),
    )
}

/// Verify pass (SPEC §9): for every HIGH/MEDIUM finding without a verdict,
/// one fresh read-only conversation decides CONFIRMED/RETRACTED. Two attempts;
/// unparseable twice → Unverified. findings.json is saved after each verdict
/// so an interrupted verify pass resumes. LOW/INFO are never verified.
fn run_verify_pass(
    root: &std::path::Path,
    output_dir: &std::path::Path,
    findings_store: &mut FindingsStore,
) {
    // Collect target indices up front so the loop body can mutate + save the
    // store without holding a borrow over it (and to dodge needless_range_loop).
    let targets: Vec<usize> = findings_store
        .findings
        .iter()
        .enumerate()
        .filter(|(_, f)| needs_verify(f))
        .map(|(i, _)| i)
        .collect();
    if targets.is_empty() {
        return;
    }
    let pending = targets.len();
    eprintln!("Verifying {pending} HIGH/MEDIUM findings...");
    let system_prompt =
        vec![RESEARCH_VERIFY_BRIEFING.replace("{root}", &root.display().to_string())];
    for (done, idx) in targets.into_iter().enumerate() {
        let finding = findings_store.findings[idx].finding.clone();
        let user = verify_prompt(&finding);
        let mut verdict = None;
        for _ in 0..2 {
            let session = Session::new();
            let mut runtime = build_research_runtime(session, system_prompt.clone());
            let text =
                match crate::brain_selector::run_turn_with_fallback(&mut runtime, &user, &mut None)
                {
                    Ok(summary) => crate::run::extract_assistant_text(&summary),
                    Err(_) => String::new(),
                };
            if let Some(v) = parse_verdict(&text) {
                verdict = Some(v);
                break;
            }
        }
        let verdict = verdict.unwrap_or(Verdict::Unverified);
        findings_store.findings[idx].verdict = Some(verdict);
        findings_store.save(&output_dir.join("findings.json")).ok();
        eprintln!(
            "  [{}/{}] finding {} — {}",
            done + 1,
            pending,
            findings_store.findings[idx].id,
            verdict_label(Some(verdict)),
        );
    }
}

/// Synthesize pass (SPEC §9): one fresh conversation gets the compact findings
/// table and writes a triage-ready report. The driver writes REPORT.md as a
/// generated metadata header + the model's report body.
fn run_synthesize_pass(
    root: &std::path::Path,
    output_dir: &std::path::Path,
    manifest: &Manifest,
    findings_store: &FindingsStore,
    batches_done: usize,
    batches_skipped: usize,
) -> anyhow::Result<()> {
    eprintln!("Synthesizing report...");
    let model = crate::model_config::active().brain.model;
    let system_prompt =
        vec![RESEARCH_SYNTH_BRIEFING.replace("{root}", &root.display().to_string())];
    let user = format!(
        "Findings ({} total):\n\n{}\n\nWrite the report per your briefing.",
        findings_store.findings.len(),
        findings_table(findings_store),
    );
    let session = Session::new();
    let mut runtime = build_research_runtime(session, system_prompt);
    let body = match crate::brain_selector::run_turn_with_fallback(&mut runtime, &user, &mut None) {
        Ok(summary) => crate::run::extract_assistant_text(&summary),
        Err(_) => String::new(),
    };
    let body = if body.trim().is_empty() {
        "_(synthesis pass produced no output)_".to_string()
    } else {
        body
    };
    let header = report_metadata_header(
        root,
        &model,
        manifest,
        findings_store,
        batches_done,
        batches_skipped,
    );
    let report_path = output_dir.join("REPORT.md");
    std::fs::write(&report_path, format!("{header}\n{body}\n"))
        .map_err(|e| anyhow::anyhow!("failed to write REPORT.md: {e}"))?;
    eprintln!("Report written: {}", report_path.display());
    Ok(())
}

// ── Tests (step 9) ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("claudette-research-test-{nanos}"))
    }

    fn setup_fixture(dir: &std::path::Path) {
        let _ = fs::create_dir_all(dir);
        // Create an empty .git directory so .gitignore is honored.
        let _ = fs::create_dir_all(dir.join(".git"));
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// manifest_filters_orders_and_counts — fixture with src/a.rs, src/b.rs,
    /// README.md, Cargo.lock (excluded name), photo.png (excluded ext),
    /// .gitignore containing ignored/, and ignored/c.rs → exactly
    /// README.md, src/a.rs, src/b.rs in that order.
    #[test]
    fn manifest_filters_orders_and_counts() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::write(dir.join("README.md"), "# Hello\nLine2\nLine3\n").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        fs::write(dir.join("src/b.rs"), "fn b() {}\nfn c() {}\n").unwrap();
        fs::write(dir.join("Cargo.lock"), "").unwrap();
        fs::write(dir.join("photo.png"), "fake-png").unwrap();
        fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::write(dir.join("ignored/c.rs"), "// ignored\n").unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build should succeed");
        assert_eq!(manifest.files.len(), 3);
        assert_eq!(manifest.files[0].rel_path, "README.md");
        assert_eq!(manifest.files[1].rel_path, "src/a.rs");
        assert_eq!(manifest.files[2].rel_path, "src/b.rs");

        // README.md has 3 lines (the content is "# Hello\nLine2\nLine3\n" → 3 lines)
        assert_eq!(manifest.files[0].lines, 3);
        assert_eq!(manifest.files[1].lines, 1);
        assert_eq!(manifest.files[2].lines, 2);

        // Cargo.lock and photo.png should be filtered out (not in skipped).
        assert!(manifest.skipped.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn parse_exclude_list_normalizes() {
        assert_eq!(
            parse_exclude_list("docs/archive, plans ,,src\\x/"),
            vec!["docs/archive", "plans", "src/x"]
        );
        assert!(parse_exclude_list("").is_empty());
        assert!(parse_exclude_list("  , ").is_empty());
    }

    #[test]
    fn path_is_excluded_matches_rules() {
        let ex = vec!["docs/archive".to_string()];
        // Path-prefix default: the archive tree is out, live docs stay in.
        assert!(path_is_excluded("docs/archive/old.md", &ex));
        assert!(path_is_excluded("docs/archive", &ex));
        assert!(!path_is_excluded("docs/guide.md", &ex));
        // docs/archive is a full path, not a bare name — a src/archive/ module
        // is untouched.
        assert!(!path_is_excluded("src/archive/mod.rs", &ex));

        // Bare-name entry matches any path segment (a dir or the file itself)
        // but not a longer segment that merely starts with it.
        let bare = vec!["archive".to_string(), "harmony.rs".to_string()];
        assert!(path_is_excluded("src/archive/x.rs", &bare));
        assert!(path_is_excluded("crates/c/src/api/harmony.rs", &bare));
        assert!(!path_is_excluded("src/archive.rs", &bare));
        assert!(!path_is_excluded("src/harmony_impl.rs", &bare));
    }

    #[test]
    fn content_has_control_tokens_detects_dense_tokens() {
        assert!(content_has_control_tokens(
            "<|channel|>analysis<|message|>real<|end|>"
        ));
        assert!(!content_has_control_tokens("if x < y && y > z"));
        assert!(!content_has_control_tokens(
            "<div><span>plain markup</span>"
        ));
        // Below the threshold of three.
        assert!(!content_has_control_tokens("one <| and <| only two"));
    }

    /// Excluded files are recorded in `skipped` (reason "excluded"), absent from
    /// `files`, while everything else is kept.
    #[test]
    fn manifest_excludes_recorded_not_dropped() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::write(dir.join("keep.rs"), "fn keep() {}\n").unwrap();
        fs::create_dir_all(dir.join("docs/archive")).unwrap();
        fs::write(dir.join("docs/archive/old.md"), "# stale\n").unwrap();

        let excludes = vec!["docs/archive".to_string()];
        let manifest =
            build_manifest_with_excludes(&dir, DEFAULT_BATCH_FILES, &excludes).expect("build");

        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].rel_path, "keep.rs");
        assert_eq!(manifest.skipped.len(), 1);
        assert_eq!(manifest.skipped[0].rel_path, "docs/archive/old.md");
        assert_eq!(manifest.skipped[0].reason, "excluded");

        cleanup(&dir);
    }

    /// A file dense with chat-template control tokens is flagged (the 2-arg
    /// wrapper applies no excludes, so the file is still kept).
    #[test]
    fn manifest_flags_control_token_files() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::write(dir.join("clean.rs"), "fn clean() {}\n").unwrap();
        fs::write(
            dir.join("tokens.rs"),
            "// leaks <|channel|>thought<|message|> and <|end|>\n",
        )
        .unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build");
        assert_eq!(manifest.flagged_control_tokens, vec!["tokens.rs"]);

        cleanup(&dir);
    }

    /// manifest_skips_oversize_and_unreadable — a >256 KB .rs file and an
    /// invalid-UTF-8 .rs file both land in skipped with the right reasons.
    #[test]
    fn manifest_skips_oversize_and_unreadable() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        // One valid file so the build succeeds (zero kept files is an error).
        fs::write(dir.join("ok.rs"), "fn ok() {}\n").unwrap();

        // >256 KB file
        let large_content = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        fs::write(dir.join("large.rs"), large_content).unwrap();

        // Invalid UTF-8 file
        fs::write(dir.join("bad.rs"), [0xFF, 0xFE]).unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build should succeed");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].rel_path, "ok.rs");
        assert_eq!(manifest.skipped.len(), 2);

        // Check reasons — order doesn't matter for the assertion.
        let reasons: Vec<&str> = manifest.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert!(reasons.contains(&"oversize"));
        assert!(reasons.contains(&"unreadable"));

        // Skipped files appear in NO batch.
        assert_eq!(manifest.batches.len(), 1);
        assert_eq!(manifest.batches[0].files, vec!["ok.rs".to_string()]);

        cleanup(&dir);
    }

    /// batching_respects_file_cap — 5 small files in one dir, max_batch_files=3 → batches of 3 then 2.
    #[test]
    fn batching_respects_file_cap() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.rs")), "x\n").unwrap();
        }

        let manifest = build_manifest(&dir, 3).expect("build should succeed");
        assert_eq!(manifest.batches.len(), 2);
        assert_eq!(manifest.batches[0].files.len(), 3);
        assert_eq!(manifest.batches[1].files.len(), 2);

        cleanup(&dir);
    }

    /// batching_respects_byte_budget — three 20 KB files in one dir → first batch 2, second batch 1.
    #[test]
    fn batching_respects_byte_budget() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        for i in 0..3 {
            fs::write(dir.join(format!("f{i}.rs")), vec![b'a'; 20 * 1024]).unwrap();
        }

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build should succeed");
        assert_eq!(manifest.batches.len(), 2);
        // First batch: 2 files (40 KB), second: 1 file (20 KB) — total would be 60 KB > 48 KB budget.
        assert_eq!(manifest.batches[0].files.len(), 2);
        assert_eq!(manifest.batches[1].files.len(), 1);

        cleanup(&dir);
    }

    /// budget_oversize_file_gets_solo_batch — a 60 KB file between small siblings → its own single-file batch.
    #[test]
    fn budget_oversize_file_gets_solo_batch() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        // Small files before and after the large one.
        fs::write(dir.join("a.rs"), "x\n").unwrap();
        fs::write(dir.join("b.rs"), vec![b'a'; 60 * 1024]).unwrap();
        fs::write(dir.join("c.rs"), "y\n").unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build should succeed");
        // a.rs + b.rs would exceed budget → a gets solo batch. b gets its own solo (60 KB > 48 KB). c gets solo.
        assert_eq!(manifest.batches.len(), 3);

        cleanup(&dir);
    }

    /// batching_breaks_at_directory_change — a/one.rs + b/two.rs → two batches even though both fit one.
    #[test]
    fn batching_breaks_at_directory_change() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::create_dir_all(dir.join("a")).unwrap();
        fs::write(dir.join("a/one.rs"), "x\n").unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("b/two.rs"), "y\n").unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build should succeed");
        assert_eq!(manifest.batches.len(), 2);
        assert_eq!(manifest.batches[0].files.len(), 1);
        assert_eq!(manifest.batches[1].files.len(), 1);

        cleanup(&dir);
    }

    /// batch_files_clamped — max_batch_files=0 and =99 behave as 1 and 8.
    #[test]
    fn batch_files_clamped() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.rs")), "x\n").unwrap();
        }

        // max_batch_files=0 → clamped to 1.
        let m0 = build_manifest(&dir, 0).expect("clamped to 1 should succeed");
        assert_eq!(m0.batches.len(), 5); // each file solo.

        // max_batch_files=99 → clamped to 8 (all 5 fit in one batch).
        let m99 = build_manifest(&dir, 99).expect("clamped to 8 should succeed");
        assert_eq!(m99.batches.len(), 1);

        cleanup(&dir);
    }

    /// empty_repo_errors — fixture with only a .png → Err.
    #[test]
    fn empty_repo_errors() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("photo.png"), "data").unwrap();

        let result = build_manifest(&dir, DEFAULT_BATCH_FILES);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no reviewable files"));

        cleanup(&dir);
    }

    /// manifest_hash_stable_and_sensitive — two builds over the same fixture → identical hash; growing one file → different hash.
    #[test]
    fn manifest_hash_stable_and_sensitive() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::write(dir.join("a.rs"), "x\n").unwrap();
        let m1 = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("first build");
        let h1 = m1.hash.clone();

        // Second build over same fixture → identical hash.
        let m2 = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("second build");
        assert_eq!(m2.hash, h1);

        // Grow one file → different hash.
        fs::write(dir.join("a.rs"), "xy\n").unwrap();
        let m3 = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("third build");
        assert_ne!(m3.hash, h1);

        cleanup(&dir);
    }

    /// parse_golden_output — preamble chatter, then a verdict, then two findings (one with multi-line evidence, one with file: lacking :line suffix and severity: high in lowercase) → everything lands, line == None on the second, dropped empty. Fixture files must exist.
    #[test]
    fn parse_golden_output() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        // Create a fixture file so the path check passes.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/good.rs"), "fn main() {}\n").unwrap();

        let output = r"Some preamble chatter that should be ignored.

### BATCH VERDICT
All files look good, no major issues found.

### FINDING
file: src/good.rs
severity: high
category: bug
claim: Missing error handling in main
evidence: The main function doesn't handle any errors.
It unwraps everything it touches.
failure: Potential panic on invalid input

### FINDING
file: src/good.rs
severity: high
category: security
claim: No input validation
evidence: User inputs are not validated before processing.
failure: Could lead to undefined behavior
";

        let parsed = parse_batch_output(output, &dir).expect("should parse");
        assert_eq!(
            parsed.verdict,
            "All files look good, no major issues found."
        );
        assert_eq!(parsed.findings.len(), 2);
        assert_eq!(parsed.dropped.len(), 0);

        // First finding: multi-line evidence joined with \n.
        let f1 = &parsed.findings[0];
        assert_eq!(f1.file, "src/good.rs");
        assert_eq!(f1.line, None);
        assert_eq!(f1.severity, Severity::High);
        assert_eq!(f1.category, Category::Bug);
        assert_eq!(
            f1.evidence,
            "The main function doesn't handle any errors.\nIt unwraps everything it touches."
        );
        assert_eq!(f1.failure, "Potential panic on invalid input");

        // Second finding: also src/good.rs, line None (no :line suffix)
        let f2 = &parsed.findings[1];
        assert_eq!(f2.file, "src/good.rs");
        assert_eq!(f2.line, None);
        assert_eq!(f2.severity, Severity::High);
        assert_eq!(f2.category, Category::Security);

        cleanup(&dir);
    }

    /// parse_missing_verdict_errors — no BATCH VERDICT section → Err.
    #[test]
    fn parse_missing_verdict_errors() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        let result = parse_batch_output("# FINDING\nfile: a.rs\nseverity: high\ncategory: bug\nclaim: test\nevidence: ev\nfailure: fail", &dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("BATCH VERDICT"));

        cleanup(&dir);
    }

    /// parse_empty_verdict_errors — empty verdict body → Err.
    #[test]
    fn parse_empty_verdict_errors() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        let result = parse_batch_output("### BATCH VERDICT\n\n### FINDING\nfile: a.rs\nseverity: high\ncategory: bug\nclaim: test\nevidence: ev\nfailure: fail", &dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));

        cleanup(&dir);
    }

    /// parse_missing_key_errors — a finding without failure: → Err mentioning failure.
    #[test]
    fn parse_missing_key_errors() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        let result = parse_batch_output(
            "### BATCH VERDICT\nok\n### FINDING\nfile: a.rs\nseverity: high\ncategory: bug\nclaim: test\nevidence: ev",
            &dir,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failure"));

        cleanup(&dir);
    }

    /// parse_bad_enum_errors — severity: CRITICAL → Err.
    #[test]
    fn parse_bad_enum_errors() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        let result = parse_batch_output(
            "### BATCH VERDICT\nok\n### FINDING\nfile: a.rs\nseverity: CRITICAL\ncategory: bug\nclaim: test\nevidence: ev\nfailure: fail",
            &dir,
        );
        assert!(result.is_err());

        cleanup(&dir);
    }

    /// parse_nonexistent_path_dropped_not_fatal — one good finding + one naming a file that doesn't exist → Ok, 1 finding, 1 dropped.
    #[test]
    fn parse_nonexistent_path_dropped_not_fatal() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        // Create the good file.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/good.rs"), "x\n").unwrap();

        let output = r"### BATCH VERDICT
ok

### FINDING
file: src/good.rs
severity: high
category: bug
claim: good finding
evidence: it works
failure: nothing

### FINDING
file: nonexistent.rs
severity: low
category: smell
claim: bad path
evidence: file not found
failure: n/a
";

        let parsed = parse_batch_output(output, &dir).expect("should succeed");
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.dropped.len(), 1);
        assert!(parsed.dropped[0].contains("nonexistent"));

        cleanup(&dir);
    }

    /// parse_traversal_and_absolute_dropped — ../secret.rs and /etc/passwd both dropped, never Err.
    #[test]
    fn parse_traversal_and_absolute_dropped() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::write(dir.join("ok.rs"), "x\n").unwrap();

        let output = r"### BATCH VERDICT
ok

### FINDING
file: ../secret.rs
severity: high
category: security
claim: traversal attempt
evidence: .. in path
failure: n/a

### FINDING
file: /etc/passwd
severity: medium
category: security
claim: absolute path
evidence: starts with /
failure: n/a
";

        let parsed = parse_batch_output(output, &dir).expect("should succeed");
        assert_eq!(parsed.findings.len(), 0);
        assert_eq!(parsed.dropped.len(), 2);

        cleanup(&dir);
    }

    /// parse_backslash_path_normalized — src\a.rs:7 → file src/a.rs, line Some(7).
    #[test]
    fn parse_backslash_path_normalized() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src").join("a.rs"), "x\n").unwrap();

        let output = r"### BATCH VERDICT
ok

### FINDING
file: src\a.rs:7
severity: low
category: smell
claim: backslash path
evidence: Windows path
failure: n/a
";

        let parsed = parse_batch_output(output, &dir).expect("should succeed");
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.findings[0].file, "src/a.rs");
        assert_eq!(parsed.findings[0].line, Some(7));

        cleanup(&dir);
    }

    /// progress_roundtrip_and_next_pending — new → save → load → equal; mark batch 1 Done → next_pending() == Some(2); all done → None; load of an absent path → Ok(None).
    #[test]
    fn progress_roundtrip_and_next_pending() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        // Create a minimal manifest.
        fs::write(dir.join("a.rs"), "x\n").unwrap();
        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build");
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        let progress = Progress::new(&manifest, started);
        assert_eq!(progress.batches.len(), 1);
        assert_eq!(progress.next_pending(), Some(1));

        // Save and load.
        let path = dir.join("progress.json");
        progress.save(&path).expect("save");
        let loaded = Progress::load(&path).expect("load").expect("should exist");
        assert_eq!(loaded.manifest_hash, progress.manifest_hash);
        assert_eq!(loaded.started_unix, started);

        // Mark batch 1 done.
        let mut progress2 = loaded;
        progress2.batches[0].state = BatchState::Done;
        progress2.save(&path).expect("save");
        let loaded2 = Progress::load(&path).expect("load").expect("should exist");
        assert_eq!(loaded2.next_pending(), None);

        // Load of absent path → Ok(None).
        let absent = dir.join("nonexistent.json");
        assert!(Progress::load(&absent).expect("load absent").is_none());

        cleanup(&dir);
    }

    /// findings_store_ids_continue_across_batches — append 2 findings for batch 1, then 1 for batch 2 → ids 1,2,3; save/load roundtrip equal.
    #[test]
    fn findings_store_ids_continue_across_batches() {
        let dir = fixture_dir();
        setup_fixture(&dir);

        let mut store = FindingsStore::new();
        // Create dummy findings.
        let f1 = Finding {
            file: "a.rs".to_string(),
            line: None,
            severity: Severity::High,
            category: Category::Bug,
            claim: "c1".into(),
            evidence: "e1".into(),
            failure: "f1".into(),
        };
        let f2 = Finding {
            file: "b.rs".to_string(),
            line: Some(5),
            severity: Severity::Medium,
            category: Category::Smell,
            claim: "c2".into(),
            evidence: "e2".into(),
            failure: "f2".into(),
        };

        store.append_batch(1, &[f1, f2]);
        let f3 = Finding {
            file: "a.rs".to_string(),
            line: None,
            severity: Severity::Low,
            category: Category::DeadCode,
            claim: "c3".into(),
            evidence: "e3".into(),
            failure: "f3".into(),
        };
        store.append_batch(2, std::slice::from_ref(&f3));

        assert_eq!(store.findings.len(), 3);
        assert_eq!(store.findings[0].id, 1);
        assert_eq!(store.findings[1].id, 2);
        assert_eq!(store.findings[2].id, 3);
        assert_eq!(store.findings[0].batch, 1);
        assert_eq!(store.findings[1].batch, 1);
        assert_eq!(store.findings[2].batch, 2);

        // Roundtrip.
        let path = dir.join("findings.json");
        store.save(&path).expect("save");
        let loaded = FindingsStore::load(&path)
            .expect("load")
            .expect("should exist");
        assert_eq!(loaded.findings.len(), 3);
        assert_eq!(loaded.findings[0].id, 1);
        assert_eq!(loaded.findings[2].id, 3);

        cleanup(&dir);
    }

    /// batch_prompt_lists_files_and_counts — two files → both path (N) lines present, header shows Batch 1/5, focus line present iff focus non-empty.
    #[test]
    fn batch_prompt_lists_files_and_counts() {
        let files = vec![
            ("src/main.rs".to_string(), 42),
            ("src/lib.rs".to_string(), 37),
        ];

        // With focus.
        let with_focus = batch_prompt(1, 5, &files, "focus on error handling");
        assert!(with_focus.contains("Batch 1/5"));
        assert!(with_focus.contains("src/main.rs (42 lines)"));
        assert!(with_focus.contains("src/lib.rs (37 lines)"));
        assert!(with_focus.contains("focus on error handling"));

        // Without focus.
        let no_focus = batch_prompt(1, 5, &files, "");
        assert!(no_focus.contains("Batch 1/5"));
        assert!(no_focus.contains("src/main.rs (42 lines)"));
        assert!(!no_focus.contains("focus on error handling"));
    }

    /// findings_md_header_and_append_format — header names the root + counts; a batch block round-trips the verdict + a finding.
    #[test]
    fn findings_md_header_and_append_format() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build");
        let header = findings_md_run_header(&dir, &dir.join("output"), &manifest);
        assert!(header.contains("Deep Research Findings"));
        assert!(header.contains("**Files reviewed:** 1"));

        // Build a finding.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/good.rs"), "fn main() {}\n").unwrap();

        let output = r"### BATCH VERDICT
All good.

### FINDING
file: src/good.rs
severity: high
category: bug
claim: test claim
evidence: the code
failure: bad thing happens";

        let parsed = parse_batch_output(output, &dir).expect("parse");
        let files = vec![("src/good.rs".to_string(), 1)];
        let block = findings_md_batch_block(2, &files, &parsed);
        assert!(block.contains("Batch 2"));
        assert!(block.contains("All good."));
        assert!(block.contains("test claim"));

        cleanup(&dir);
    }

    /// classify_attempt_empty_parse_ok — empty → Empty; valid → Parsed; bad format → ParseError.
    #[test]
    fn classify_attempt_empty_parse_ok() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();

        assert!(matches!(classify_attempt("", &dir), AttemptOutcome::Empty));
        assert!(matches!(
            classify_attempt("   \n  ", &dir),
            AttemptOutcome::Empty
        ));

        let valid = r"### BATCH VERDICT
ok";
        match classify_attempt(valid, &dir) {
            AttemptOutcome::Parsed(pb) => assert_eq!(pb.verdict, "ok"),
            other => panic!("expected Parsed, got {:?}", other),
        }

        match classify_attempt("not a verdict", &dir) {
            AttemptOutcome::ParseError(_) => {} // expected
            other => panic!("expected ParseError, got {:?}", other),
        }

        cleanup(&dir);
    }

    /// resume_action_matrix — covers all four outcomes.
    #[test]
    fn resume_action_matrix() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        // Four files at 3-per-batch → two batches, so a resumed run has a
        // pending batch 2 once batch 1 is marked done.
        for name in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            fs::write(dir.join(name), "x\n").unwrap();
        }
        let manifest = build_manifest(&dir, DEFAULT_BATCH_FILES).expect("build");

        // No existing progress → Fresh.
        assert_eq!(resume_action(None, &manifest.hash), ResumeAction::Fresh);

        // Hash mismatch → RefuseChanged.
        let prog = Progress::new(&manifest, 0);
        assert_eq!(
            resume_action(Some(&prog), "different_hash"),
            ResumeAction::RefuseChanged
        );

        // Phase Done → RefuseDone.
        let mut done_prog = prog;
        done_prog.phase = Phase::Done;
        assert_eq!(
            resume_action(Some(&done_prog), &manifest.hash),
            ResumeAction::RefuseDone
        );

        // Phase Batches, pending batch exists → ResumeAt.
        let mut resume_prog = Progress::new(&manifest, 0);
        resume_prog.batches[0].state = BatchState::Done;
        assert_eq!(
            resume_action(Some(&resume_prog), &manifest.hash),
            ResumeAction::ResumeAt(2)
        );

        cleanup(&dir);
    }

    /// output_dir_default_and_override — override honored as-is; default ends with <repo>-<date>.
    #[test]
    fn output_dir_default_and_override() {
        let _guard = crate::test_env_lock();

        // Override.
        std::env::set_var("CLAUDETTE_RESEARCH_DIR", "/custom/output");
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::write(dir.join("a.rs"), "x\n").unwrap();
        assert_eq!(
            resolve_output_dir(&dir).expect("override"),
            std::path::PathBuf::from("/custom/output")
        );

        // Default.
        std::env::remove_var("CLAUDETTE_RESEARCH_DIR");
        let result = resolve_output_dir(&dir).expect("default");
        let result_str = result.to_string_lossy();
        assert!(result_str.contains(".claudette"));
        assert!(result_str.contains("research"));

        cleanup(&dir);
    }

    /// force_offline_for_run_sets_env — unset → becomes "1"; a pre-set value is left untouched.
    #[test]
    fn force_offline_for_run_sets_env() {
        let _guard = crate::test_env_lock();

        // Unset → set to "1".
        std::env::remove_var(crate::egress::OFFLINE_ENV);
        force_offline_for_run();
        assert_eq!(
            std::env::var(crate::egress::OFFLINE_ENV).ok(),
            Some("1".to_string())
        );

        // Pre-set value left untouched.
        std::env::set_var(crate::egress::OFFLINE_ENV, "0");
        force_offline_for_run();
        assert_eq!(
            std::env::var(crate::egress::OFFLINE_ENV).ok(),
            Some("0".to_string())
        );

        // Clean up.
        std::env::remove_var(crate::egress::OFFLINE_ENV);
    }

    /// force_workspace_for_run — unset → set to the root; a pre-set value is
    /// left untouched. Restores the original value either way (the dev shell
    /// often has CLAUDETTE_WORKSPACE exported — never leak a change).
    #[test]
    fn force_workspace_for_run_sets_env() {
        let _guard = crate::test_env_lock();
        let original = std::env::var("CLAUDETTE_WORKSPACE").ok();

        // Unset → set to the root.
        std::env::remove_var("CLAUDETTE_WORKSPACE");
        force_workspace_for_run(std::path::Path::new("/tmp/research-target"));
        assert_eq!(
            std::env::var("CLAUDETTE_WORKSPACE").ok().as_deref(),
            Some("/tmp/research-target")
        );

        // Pre-set value left untouched.
        std::env::set_var("CLAUDETTE_WORKSPACE", "/already/set");
        force_workspace_for_run(std::path::Path::new("/tmp/research-target"));
        assert_eq!(
            std::env::var("CLAUDETTE_WORKSPACE").ok().as_deref(),
            Some("/already/set")
        );

        // Restore the original environment.
        match original {
            Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
            None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
        }
    }

    /// parse_finding_with_line_range — `file: path:6-7` (range) is kept, not
    /// dropped as an unsafe path; the line is the start of the range.
    #[test]
    fn parse_finding_with_line_range() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/math.rs"), "x\n").unwrap();

        let output = "### BATCH VERDICT\nok\n### FINDING\nfile: src/math.rs:6-7\nseverity: high\ncategory: bug\nclaim: c\nevidence: e\nfailure: f";
        let parsed = parse_batch_output(output, &dir).expect("parse");
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.findings[0].file, "src/math.rs");
        assert_eq!(parsed.findings[0].line, Some(6));
        assert!(parsed.dropped.is_empty());

        cleanup(&dir);
    }

    /// parse_evidence_strips_code_fences — a fenced ```rust … ``` evidence block
    /// is captured without the fence markers.
    #[test]
    fn parse_evidence_strips_code_fences() {
        let dir = fixture_dir();
        setup_fixture(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/math.rs"), "x\n").unwrap();

        let output = "### BATCH VERDICT\nok\n### FINDING\nfile: src/math.rs:6\nseverity: high\ncategory: bug\nclaim: c\nevidence:\n```rust\nlet x = 1;\n```\nfailure: f";
        let parsed = parse_batch_output(output, &dir).expect("parse");
        assert_eq!(parsed.findings.len(), 1);
        assert!(!parsed.findings[0].evidence.contains("```"));
        assert!(parsed.findings[0].evidence.contains("let x = 1;"));

        cleanup(&dir);
    }

    #[test]
    fn parse_verdict_confirmed_retracted_none() {
        assert_eq!(
            parse_verdict("VERDICT: CONFIRMED\nreason: holds"),
            Some(Verdict::Confirmed)
        );
        assert_eq!(
            parse_verdict("chatter\nVERDICT: retracted\nreason: nope"),
            Some(Verdict::Retracted)
        );
        assert_eq!(parse_verdict("no verdict line here"), None);
    }

    #[test]
    fn needs_verify_high_medium_only() {
        let mk = |sev: Severity, v: Option<Verdict>| StoredFinding {
            id: 1,
            batch: 1,
            finding: Finding {
                file: "a.rs".into(),
                line: None,
                severity: sev,
                category: Category::Bug,
                claim: "c".into(),
                evidence: "e".into(),
                failure: "f".into(),
            },
            verdict: v,
        };
        assert!(needs_verify(&mk(Severity::High, None)));
        assert!(needs_verify(&mk(Severity::Medium, None)));
        assert!(!needs_verify(&mk(Severity::Low, None)));
        assert!(!needs_verify(&mk(Severity::Info, None)));
        assert!(!needs_verify(&mk(Severity::High, Some(Verdict::Confirmed))));
    }

    #[test]
    fn findings_table_lists_rows() {
        let mut store = FindingsStore::new();
        store.append_batch(
            1,
            &[Finding {
                file: "src/a.rs".into(),
                line: Some(3),
                severity: Severity::High,
                category: Category::Bug,
                claim: "boom".into(),
                evidence: "e".into(),
                failure: "f".into(),
            }],
        );
        store.findings[0].verdict = Some(Verdict::Confirmed);
        let table = findings_table(&store);
        assert!(table.contains("| id |"));
        assert!(table.contains("src/a.rs:3"));
        assert!(table.contains("confirmed"));
    }

    #[test]
    fn report_header_names_root_model_and_counts() {
        let manifest = Manifest {
            root: "r".into(),
            files: vec![],
            skipped: vec![],
            batches: vec![],
            hash: "h".into(),
            flagged_control_tokens: vec![],
        };
        let mut store = FindingsStore::new();
        store.append_batch(
            1,
            &[Finding {
                file: "a.rs".into(),
                line: None,
                severity: Severity::High,
                category: Category::Bug,
                claim: "c".into(),
                evidence: "e".into(),
                failure: "f".into(),
            }],
        );
        store.findings[0].verdict = Some(Verdict::Confirmed);
        let h = report_metadata_header(
            std::path::Path::new("/tmp/repo"),
            "test-model",
            &manifest,
            &store,
            5,
            1,
        );
        assert!(h.contains("Deep Research Report"));
        assert!(h.contains("test-model"));
        assert!(h.contains("1 HIGH"));
        assert!(h.contains("1 confirmed"));
    }

    #[test]
    fn verify_prompt_carries_finding() {
        let f = Finding {
            file: "src/x.rs".into(),
            line: Some(9),
            severity: Severity::Medium,
            category: Category::Security,
            claim: "unchecked input".into(),
            evidence: "e".into(),
            failure: "boom scenario".into(),
        };
        let p = verify_prompt(&f);
        assert!(p.contains("src/x.rs:9"));
        assert!(p.contains("unchecked input"));
        assert!(p.contains("boom scenario"));
    }

    #[test]
    fn probe_healthy_rejects_whitespace() {
        assert!(!probe_healthy("  \n\t"));
        assert!(probe_healthy("OK"));
    }

    #[test]
    fn flip_skipped_to_pending_flips_only_skipped() {
        let mut progress = Progress {
            manifest_hash: "h".to_string(),
            started_unix: 0,
            phase: Phase::Batches,
            batches: vec![
                BatchStatus {
                    id: 1,
                    state: BatchState::Done,
                    attempts: 1,
                    findings: 2,
                    wall_secs: 10,
                    skip_reason: None,
                },
                BatchStatus {
                    id: 2,
                    state: BatchState::Skipped,
                    attempts: 2,
                    findings: 0,
                    wall_secs: 99,
                    skip_reason: Some("findings did not parse: x".to_string()),
                },
                BatchStatus {
                    id: 3,
                    state: BatchState::Pending,
                    attempts: 0,
                    findings: 0,
                    wall_secs: 0,
                    skip_reason: None,
                },
            ],
        };
        assert_eq!(flip_skipped_to_pending(&mut progress), 1);
        assert_eq!(progress.batches[0].state, BatchState::Done);
        assert_eq!(progress.batches[1].state, BatchState::Pending);
        assert_eq!(progress.batches[1].attempts, 0);
        assert_eq!(progress.batches[1].skip_reason, None);
        assert_eq!(progress.batches[2].state, BatchState::Pending);
    }

    #[test]
    fn batch_status_skip_reason_defaults_to_none() {
        let json = r#"{"id":1,"state":"done","attempts":1,"findings":2,"wall_secs":30}"#;
        let s: BatchStatus = serde_json::from_str(json).unwrap();
        assert_eq!(s.skip_reason, None);
    }

    #[test]
    fn skipped_note_includes_reason() {
        let note = findings_md_skipped_note(7, "findings did not parse: bad enum");
        assert!(note.contains("Batch 7"));
        assert!(note.contains("bad enum"));
    }
}
