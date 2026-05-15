//! `claudette --doctor` — diagnostic probe of every external dependency.
//!
//! Resolves every `CLAUDETTE_*` env var the runtime cares about, then prints
//! green/red status lines for: Ollama / LM Studio reachable, the configured
//! brain model pulled, the embed endpoint + recall model loaded, Google
//! OAuth tokens valid for each configured scope, `ffmpeg` / `whisper-cli`
//! on PATH, and the `~/.claudette/secrets/*` token files.
//!
//! This is intentionally a flat probe — each check is independent, no probe
//! short-circuits the others. The user wants to see *everything* at once;
//! one broken row shouldn't hide a different broken row underneath.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::theme;

/// Outcome of one diagnostic probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Err,
}

impl Status {
    fn glyph(self) -> &'static str {
        match self {
            Self::Ok => theme::OK_GLYPH,
            Self::Warn => theme::WARN_GLYPH,
            Self::Err => theme::ERR_GLYPH,
        }
    }
}

fn print_row(label: &str, status: Status, detail: &str) {
    let glyph = status.glyph();
    let painted = match status {
        Status::Ok => theme::ok(label).to_string(),
        Status::Warn => theme::warn(label).to_string(),
        Status::Err => theme::error(label).to_string(),
    };
    if detail.is_empty() {
        eprintln!("  {glyph} {painted}");
    } else {
        eprintln!("  {glyph} {painted}  {}", theme::dim(detail));
    }
}

fn print_section(title: &str) {
    eprintln!();
    eprintln!("{}", theme::accent(title));
}

fn home_dir() -> PathBuf {
    let raw = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(raw)
}

fn claudette_home() -> PathBuf {
    home_dir().join(".claudette")
}

/// Entry point — runs every probe and returns the exit code for the CLI.
/// Returns `0` when nothing is `Err` (warnings are allowed), `1` otherwise.
pub fn run() -> i32 {
    theme::init();
    eprintln!(
        "{} {}",
        theme::GEAR,
        theme::brand(&format!(
            "claudette --doctor (v{})",
            env!("CARGO_PKG_VERSION")
        ))
    );

    let mut any_err = false;
    let mut bump = |s: Status| {
        if s == Status::Err {
            any_err = true;
        }
    };

    print_section("environment");
    bump(probe_env());

    print_section("local brain");
    bump(probe_brain());

    print_section("recall / embeddings");
    bump(probe_recall());

    print_section("google oauth");
    bump(probe_google_oauth());

    print_section("voice (optional)");
    bump(probe_voice());

    print_section("secrets directory");
    bump(probe_secrets());

    eprintln!();
    if any_err {
        eprintln!(
            "{} {}",
            theme::ERR_GLYPH,
            theme::error("one or more probes failed — see red rows above")
        );
        1
    } else {
        eprintln!("{} {}", theme::OK_GLYPH, theme::ok("all probes passed"));
        0
    }
}

// ─── Env vars ────────────────────────────────────────────────────────────

/// Every `CLAUDETTE_*` env var the runtime reads. Kept as a flat list so
/// the doctor view stays scannable. Names mirror their source-file
/// definitions; values are printed verbatim (no redaction) — these are
/// configuration knobs, not secrets.
const TRACKED_VARS: &[&str] = &[
    // Top-level switches
    "CLAUDETTE_MODEL",
    "CLAUDETTE_FALLBACK_BRAIN_MODEL",
    "CLAUDETTE_CODER_MODEL",
    "CLAUDETTE_NUM_CTX",
    "CLAUDETTE_NUM_PREDICT",
    "CLAUDETTE_MAX_ITERATIONS",
    "CLAUDETTE_MAX_FIX_ROUNDS",
    "CLAUDETTE_SESSION",
    "CLAUDETTE_WORKSPACE",
    // Compaction & resilience
    "CLAUDETTE_COMPACT_THRESHOLD",
    "CLAUDETTE_SOFT_COMPACT_THRESHOLD",
    "CLAUDETTE_MODEL_RELOAD_RETRY_MS",
    "CLAUDETTE_DISABLE_MODEL_RELOAD_RETRY",
    // Backends
    "OLLAMA_HOST",
    "CLAUDETTE_OPENAI_COMPAT",
    "CLAUDETTE_ALLOW_REMOTE_OLLAMA",
    "CLAUDETTE_SKIP_OLLAMA_PROBE",
    "CLAUDETTE_SKIP_LM_STUDIO_PROBE",
    "CLAUDETTE_MAX_TOOLS",
    // Recall
    "CLAUDETTE_RECALL_MODEL",
    "CLAUDETTE_RECALL_DB",
    "CLAUDETTE_RECALL_DISABLE",
    // Voice
    "CLAUDETTE_FFMPEG_BIN",
    "CLAUDETTE_WHISPER_BIN",
    "CLAUDETTE_WHISPER_MODEL",
    // Integrations
    "TELEGRAM_BOT_TOKEN",
    "CLAUDETTE_TELEGRAM_CHAT",
    "GITHUB_TOKEN",
    "BRAVE_API_KEY",
    "CLAUDETTE_GOOGLE_CLIENT_ID",
    "GOOGLE_CLIENT_ID",
];

fn probe_env() -> Status {
    let mut set_count = 0;
    for var in TRACKED_VARS {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                set_count += 1;
                let preview = redact_for_display(var, &val);
                print_row(var, Status::Ok, &preview);
            }
        }
    }
    if set_count == 0 {
        print_row(
            "no CLAUDETTE_* env vars set",
            Status::Warn,
            "running with defaults; consult README.md for tunables",
        );
        return Status::Warn;
    }
    Status::Ok
}

/// Mask the value of vars whose name implies a secret. Configuration knobs
/// stay readable. The match is conservative — anything containing `TOKEN`,
/// `KEY`, `SECRET`, or `CLIENT_ID` is reduced to a length + last-4 preview.
fn redact_for_display(var: &str, val: &str) -> String {
    let upper = var.to_ascii_uppercase();
    let looks_secret = upper.contains("TOKEN")
        || upper.contains("KEY")
        || upper.contains("SECRET")
        || upper.contains("CLIENT_ID");
    if !looks_secret {
        return val.to_string();
    }
    if val.len() <= 6 {
        return "***".to_string();
    }
    let tail = &val[val.len().saturating_sub(4)..];
    format!("*** ({} chars, …{tail})", val.len())
}

// ─── Brain ───────────────────────────────────────────────────────────────

fn probe_brain() -> Status {
    let base = crate::api::resolve_ollama_url();
    let compat = is_openai_compat();
    let configured_model = crate::run::current_model();

    print_row(
        if compat {
            "backend: openai-compat"
        } else {
            "backend: ollama"
        },
        Status::Ok,
        &base,
    );

    // Reachability
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            print_row("http client", Status::Err, &format!("build failed: {e}"));
            return Status::Err;
        }
    };

    let mut overall = Status::Ok;

    let tags_url = if compat {
        format!("{base}/v1/models")
    } else {
        format!("{base}/api/tags")
    };
    let resp = client.get(&tags_url).send();
    match resp {
        Ok(r) if r.status().is_success() => {
            print_row(
                if compat {
                    "reachable: /v1/models"
                } else {
                    "reachable: /api/tags"
                },
                Status::Ok,
                &format!("HTTP {}", r.status().as_u16()),
            );
            // Parse the model list and look for the configured brain.
            let body: Value = match r.json() {
                Ok(v) => v,
                Err(e) => {
                    print_row(
                        "parse model list",
                        Status::Warn,
                        &format!("non-JSON response: {e}"),
                    );
                    return Status::Warn;
                }
            };
            let names = extract_model_names(&body, compat);
            if names.is_empty() {
                print_row(
                    "model list",
                    Status::Err,
                    "server returned an empty model list — load one first",
                );
                overall = Status::Err;
            } else if model_present(&names, &configured_model) {
                print_row(
                    &format!("brain '{configured_model}' loaded"),
                    Status::Ok,
                    &format!("{} model(s) available", names.len()),
                );
            } else {
                let hint = if compat {
                    format!(
                        "load it in LM Studio's Local Server tab (looking for: {configured_model})"
                    )
                } else {
                    format!("`ollama pull {configured_model}` to fetch it")
                };
                print_row(
                    &format!("brain '{configured_model}' NOT in model list"),
                    Status::Err,
                    &hint,
                );
                overall = Status::Err;
            }
        }
        Ok(r) => {
            print_row(
                "reachable",
                Status::Err,
                &format!("HTTP {} at {tags_url}", r.status().as_u16()),
            );
            overall = Status::Err;
        }
        Err(e) => {
            print_row(
                "reachable",
                Status::Err,
                &format!("{e} — start the server or set OLLAMA_HOST"),
            );
            overall = Status::Err;
        }
    }
    overall
}

fn is_openai_compat() -> bool {
    matches!(
        std::env::var("CLAUDETTE_OPENAI_COMPAT").ok().as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Pull model ids out of an Ollama `/api/tags` or OpenAI-compat `/v1/models`
/// response body.
fn extract_model_names(body: &Value, openai_compat: bool) -> Vec<String> {
    let arr = if openai_compat {
        body.get("data").and_then(Value::as_array)
    } else {
        body.get("models").and_then(Value::as_array)
    };
    let Some(arr) = arr else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        // Ollama: `{"name": "qwen3.5:9b", …}`. OpenAI-compat: `{"id": "…", …}`.
        let key = if openai_compat { "id" } else { "name" };
        if let Some(name) = entry.get(key).and_then(Value::as_str) {
            out.push(name.to_string());
        }
    }
    out
}

/// Loose match — accepts `qwen3:8b` ↔ `qwen3:8b-latest` etc. Both sides are
/// lowercased; the configured name matches if it's equal to OR a prefix of
/// the listed name when delimited by `:` (so the listed `qwen3:8b-q4_0`
/// satisfies a configured `qwen3:8b` only if the user spelled it that way).
fn model_present(names: &[String], wanted: &str) -> bool {
    let w = wanted.to_ascii_lowercase();
    names.iter().any(|n| {
        let n = n.to_ascii_lowercase();
        n == w || n == format!("{w}:latest") || w == format!("{n}:latest")
    })
}

// ─── Recall / embeddings ─────────────────────────────────────────────────

fn probe_recall() -> Status {
    if matches!(
        std::env::var("CLAUDETTE_RECALL_DISABLE").as_deref(),
        Ok("1")
    ) {
        print_row(
            "recall disabled by env",
            Status::Warn,
            "CLAUDETTE_RECALL_DISABLE=1 — skipping embed probe",
        );
        return Status::Warn;
    }
    match crate::recall::probe() {
        Ok(()) => {
            print_row(
                "embed probe",
                Status::Ok,
                "1-token /embeddings round-trip OK",
            );
            Status::Ok
        }
        Err(e) => {
            print_row("embed probe", Status::Err, &e);
            Status::Err
        }
    }
}

// ─── Google OAuth ────────────────────────────────────────────────────────

fn probe_google_oauth() -> Status {
    let mut worst = Status::Ok;
    for ctx in [
        crate::google_auth::AuthContext::Calendar,
        crate::google_auth::AuthContext::GmailRead,
    ] {
        let label = ctx.label();
        match crate::google_auth::access_token(ctx) {
            Err(e) => {
                let s = if e.contains("not authenticated") {
                    print_row(
                        &format!("{label}: not configured"),
                        Status::Warn,
                        &format!("run `claudette --auth-google {label}` to enable"),
                    );
                    Status::Warn
                } else {
                    print_row(&format!("{label} token"), Status::Err, &e);
                    Status::Err
                };
                if s == Status::Err {
                    worst = Status::Err;
                } else if worst == Status::Ok {
                    worst = Status::Warn;
                }
            }
            Ok(token) => {
                // Live verify with one tiny read call.
                match verify_scope(ctx, &token) {
                    Ok(detail) => print_row(&format!("{label} access"), Status::Ok, &detail),
                    Err(e) => {
                        print_row(&format!("{label} access"), Status::Err, &e);
                        worst = Status::Err;
                    }
                }
            }
        }
    }
    worst
}

/// Shared live-verify with the `--auth-google` post-grant check —
/// `crate::google_auth::verify_scope_live` is the single source of truth
/// for "does this token actually work against the API". Keeps the doctor
/// and the OAuth flow in lockstep so a passing `--doctor` row implies
/// the same thing as the "OK: ... verified" line the auth flow prints.
fn verify_scope(ctx: crate::google_auth::AuthContext, token: &str) -> Result<String, String> {
    crate::google_auth::verify_scope_live(ctx, token)
}

// ─── Voice deps ──────────────────────────────────────────────────────────

fn probe_voice() -> Status {
    let ffmpeg = std::env::var("CLAUDETTE_FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string());
    let whisper =
        std::env::var("CLAUDETTE_WHISPER_BIN").unwrap_or_else(|_| "whisper-cli".to_string());

    let ffmpeg_ok = Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    if ffmpeg_ok {
        print_row(&ffmpeg, Status::Ok, "on PATH");
    } else {
        print_row(
            &ffmpeg,
            Status::Warn,
            "not found — voice transcription disabled",
        );
    }

    let whisper_ok = Command::new(&whisper).arg("--help").output().is_ok();
    if whisper_ok {
        print_row(&whisper, Status::Ok, "on PATH");
    } else {
        print_row(
            &whisper,
            Status::Warn,
            "not found — voice transcription disabled",
        );
    }
    if ffmpeg_ok && whisper_ok {
        Status::Ok
    } else {
        Status::Warn
    }
}

// ─── Secrets dir ─────────────────────────────────────────────────────────

fn probe_secrets() -> Status {
    let dir = claudette_home().join("secrets");
    if !dir.exists() {
        print_row(
            "secrets dir",
            Status::Warn,
            &format!("{} does not exist (no tokens stored yet)", dir.display()),
        );
        return Status::Warn;
    }
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            count += 1;
            print_row(name, Status::Ok, &format!("{} bytes", meta.len()));
        }
    }
    if count == 0 {
        print_row(
            "secrets dir",
            Status::Warn,
            &format!("{} is empty", dir.display()),
        );
        return Status::Warn;
    }
    Status::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_model_names_ollama_shape() {
        let body = json!({
            "models": [
                { "name": "qwen3:8b" },
                { "name": "nomic-embed-text:latest" }
            ]
        });
        let names = extract_model_names(&body, false);
        assert_eq!(names, vec!["qwen3:8b", "nomic-embed-text:latest"]);
    }

    #[test]
    fn extract_model_names_openai_compat_shape() {
        let body = json!({
            "data": [
                { "id": "gemma-4-26b-a4b-it" },
                { "id": "text-embedding-nomic-embed-text-v1.5" }
            ]
        });
        let names = extract_model_names(&body, true);
        assert_eq!(
            names,
            vec!["gemma-4-26b-a4b-it", "text-embedding-nomic-embed-text-v1.5"]
        );
    }

    #[test]
    fn extract_model_names_returns_empty_on_unknown_shape() {
        let body = json!({ "unexpected": [] });
        assert!(extract_model_names(&body, false).is_empty());
        assert!(extract_model_names(&body, true).is_empty());
    }

    #[test]
    fn model_present_matches_latest_alias_either_direction() {
        let names = vec!["qwen3:8b".to_string()];
        assert!(model_present(&names, "qwen3:8b"));
        assert!(model_present(&names, "qwen3:8b:latest"));
        let names2 = vec!["qwen3:8b:latest".to_string()];
        assert!(model_present(&names2, "qwen3:8b"));
    }

    #[test]
    fn model_present_is_case_insensitive() {
        let names = vec!["Qwen3:8B".to_string()];
        assert!(model_present(&names, "qwen3:8b"));
    }

    #[test]
    fn model_present_rejects_mismatch() {
        let names = vec!["qwen3:8b".to_string()];
        assert!(!model_present(&names, "llama3:70b"));
    }

    #[test]
    fn redact_masks_anything_with_token_or_key_or_secret() {
        let r = redact_for_display("GITHUB_TOKEN", "ghp_abcdef123456");
        assert!(r.contains("***"), "GITHUB_TOKEN should be masked: {r}");
        assert!(r.contains("3456"));
        let r2 = redact_for_display("BRAVE_API_KEY", "bsk_supersecretvalue");
        assert!(r2.contains("***"));
    }

    #[test]
    fn redact_preserves_config_values() {
        assert_eq!(
            redact_for_display("OLLAMA_HOST", "localhost:11434"),
            "localhost:11434"
        );
        assert_eq!(redact_for_display("CLAUDETTE_NUM_CTX", "32768"), "32768");
        assert_eq!(
            redact_for_display("CLAUDETTE_MODEL", "qwen3:8b"),
            "qwen3:8b"
        );
    }

    #[test]
    fn redact_short_secret_is_fully_starred() {
        assert_eq!(redact_for_display("SOME_TOKEN", "abc"), "***");
        assert_eq!(redact_for_display("SOME_TOKEN", ""), "***");
    }
}
