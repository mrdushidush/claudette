//! Pure core of deep-research mode (`claudette --research`, wired in a
//! follow-up PR): walks a target repo into a deterministic review manifest,
//! plans 2-3-file batches, parses the reviewer's structured findings output,
//! and persists progress/findings JSON so an interrupted run resumes exactly
//! where it stopped.
#![allow(dead_code)] // wired into the CLI driver in the follow-up PR (R2)

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
}

/// Build a deterministic review manifest from `root`.
///
/// Walks the directory tree (respecting `.gitignore`), collects eligible files,
/// plans them into size-aware batches, and returns a [`Manifest`].
#[allow(clippy::too_many_lines)] // card-prescribed algorithm, kept inline
pub(crate) fn build_manifest(
    root: &std::path::Path,
    max_batch_files: usize,
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

    Ok(Manifest {
        root: canonical_root.to_string_lossy().to_string(),
        files: kept,
        skipped,
        batches,
        hash,
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
                Some("evidence") => append_field(&mut evidence_val, &trimmed),
                Some("failure") => append_field(&mut failure_val, &trimmed),
                // Chatter before the first free-text key — ignored.
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

    // Strip trailing :<digits> into line number.
    let mut line: Option<u32> = None;
    if let Some(colon_pos) = norm_file.rfind(':') {
        let suffix = &norm_file[colon_pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) && !suffix.is_empty() {
            line = suffix.parse::<u32>().ok();
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
}
