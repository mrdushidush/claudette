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

/// Print an indented, copy-pasteable remediation line under the preceding row.
/// Doctor's whole value is telling the user *exactly* what to run next, so the
/// red/yellow rows are paired with a concrete `↳ fix:` command wherever one
/// exists.
fn print_fix(cmd: &str) {
    eprintln!("      {} {}", theme::accent("↳ fix:"), theme::dim(cmd));
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

    print_section("egress / air-gap");
    bump(probe_egress());

    print_section("local brain");
    bump(probe_brain());

    print_section("pick a brain (VRAM → certified model)");
    bump(probe_pick_brain());

    print_section("build toolchains");
    bump(probe_toolchains());

    print_section("recall / embeddings");
    bump(probe_recall());

    // Google OAuth only exists in a default-features build; a coding-only
    // build (--no-default-features) has no Google code to probe.
    #[cfg(feature = "integrations")]
    {
        print_section("google oauth");
        bump(probe_google_oauth());
    }

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
    "CLAUDETTE_OFFLINE",
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
    if val.chars().count() <= 6 {
        return "***".to_string();
    }
    // Last 4 *chars*, not bytes — a byte slice at `len-4` panics when it splits
    // a multibyte glyph, and `panic="abort"` would take the whole `doctor` run
    // down. (roast 2026-06-02)
    let chars: Vec<char> = val.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("*** ({} chars, …{tail})", chars.len())
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
                print_fix(&model_load_hint(compat, &configured_model));
                overall = Status::Err;
            } else if model_present(&names, &configured_model) {
                print_row(
                    &format!("brain '{configured_model}' loaded"),
                    Status::Ok,
                    &format!("{} model(s) available", names.len()),
                );
            } else {
                print_row(
                    &format!("brain '{configured_model}' NOT in model list"),
                    Status::Err,
                    &format!("{} other model(s) available", names.len()),
                );
                print_fix(&model_load_hint(compat, &configured_model));
                overall = Status::Err;
            }
        }
        Ok(r) => {
            print_row(
                "reachable",
                Status::Err,
                &format!("HTTP {} at {tags_url}", r.status().as_u16()),
            );
            print_fix(&backend_start_hint(compat));
            overall = Status::Err;
        }
        Err(e) => {
            print_row("not reachable", Status::Err, &format!("{base} — {e}"));
            print_fix(&backend_start_hint(compat));
            overall = Status::Err;
        }
    }
    overall
}

/// VRAM-aware brain recommendation — answers "which model should I run?"
/// with the Claudette-Certified battery data instead of vibes. Advisory
/// only: never touches `CLAUDETTE_MODEL`, never evicts anything (runtime
/// selection stays with `brain_selector`). Always returns `Ok`/`Warn` —
/// a recommendation can't be a hard failure.
fn probe_pick_brain() -> Status {
    let compat = is_openai_compat();
    let configured = crate::run::current_model();
    let (vram_gb, source) = crate::hw::resolve_vram_gb();

    let status = match source {
        crate::hw::VramSource::Detected => {
            print_row(
                "gpu vram (nvidia-smi)",
                Status::Ok,
                &format!("{vram_gb:.1} GiB"),
            );
            Status::Ok
        }
        crate::hw::VramSource::EnvVar => {
            print_row(
                "gpu vram (CLAUDETTE_VRAM_GB)",
                Status::Ok,
                &format!("{vram_gb:.1} GiB — nvidia-smi unavailable, using your override"),
            );
            Status::Ok
        }
        crate::hw::VramSource::Default => {
            print_row(
                "gpu vram unknown",
                Status::Warn,
                "no nvidia-smi (AMD/Apple/CPU?) and no CLAUDETTE_VRAM_GB — assuming 8 GiB; \
                 set CLAUDETTE_VRAM_GB to your real figure for a better pick",
            );
            Status::Warn
        }
    };

    let rec = crate::hw::recommend_brain(vram_gb, compat);
    let rec_model = rec.model;
    print_row(
        &format!("recommended brain: {rec_model}"),
        Status::Ok,
        rec.why,
    );
    if !rec.alternatives.is_empty() {
        print_row("alternatives", Status::Ok, rec.alternatives);
    }
    if model_present(std::slice::from_ref(&configured), rec_model) {
        print_row("configured brain already matches", Status::Ok, &configured);
    } else {
        print_fix(&model_load_hint(compat, rec_model));
        print_row(
            "currently configured",
            Status::Ok,
            &format!("{configured} — switch with CLAUDETTE_MODEL={rec_model} (advisory; nothing is changed for you)"),
        );
    }
    status
}

/// Copy-paste command to start the model server for the active backend.
/// `pub(crate)`: shared with the first-run remediation path (`firstrun.rs`)
/// so startup and `--doctor` give the same advice.
pub(crate) fn backend_start_hint(compat: bool) -> String {
    if compat {
        "open LM Studio → Developer (Local Server) tab → Start, or run `lms server start` \
         (default http://localhost:1234)"
            .to_string()
    } else {
        "run `ollama serve` in another terminal, or set OLLAMA_HOST to your endpoint".to_string()
    }
}

/// Copy-paste command to load/pull the configured brain for the active backend.
/// `pub(crate)`: shared with the first-run remediation path (`firstrun.rs`).
pub(crate) fn model_load_hint(compat: bool, model: &str) -> String {
    if compat {
        format!(
            "load `{model}` in LM Studio (Models tab → load), or pick another with CLAUDETTE_MODEL"
        )
    } else {
        format!("ollama pull {model}")
    }
}

fn is_openai_compat() -> bool {
    matches!(
        std::env::var("CLAUDETTE_OPENAI_COMPAT").ok().as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Pull model ids out of an Ollama `/api/tags` or OpenAI-compat `/v1/models`
/// response body. `pub(crate)`: shared with `firstrun.rs` so the startup
/// classifier and `--doctor` parse the same shapes the same way.
pub(crate) fn extract_model_names(body: &Value, openai_compat: bool) -> Vec<String> {
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
/// `pub(crate)`: shared with `firstrun.rs`.
pub(crate) fn model_present(names: &[String], wanted: &str) -> bool {
    let w = wanted.to_ascii_lowercase();
    names.iter().any(|n| {
        let n = n.to_ascii_lowercase();
        n == w || n == format!("{w}:latest") || w == format!("{n}:latest")
    })
}

// ─── Build toolchains ─────────────────────────────────────────────────────
//
// forge runs the project's real build + test suite (cargo check/test, go
// build/test, pytest, npm test) inside the Verifier, and codet shells out to
// language compilers for its syntax/test checks. Missing a toolchain is the #1
// silent reason "forge says it passed but nothing actually compiled". This
// section probes each toolchain and, when one is missing, prints a copy-paste
// install command for the current OS.

/// One build-toolchain probe. `bins` is tried in order (first hit wins) so a
/// platform that ships `python3` but not `python` still resolves.
struct Toolchain {
    label: &'static str,
    /// Stable key into [`toolchain_install_hint`].
    key: &'static str,
    bins: &'static [&'static str],
    version_arg: &'static str,
    /// What in claudette needs it — shown when it's missing.
    why: &'static str,
    /// `true` ⇒ a miss is an error (claudette can't function without it);
    /// `false` ⇒ a miss is a warning (only needed for that language).
    required: bool,
}

const TOOLCHAINS: &[Toolchain] = &[
    Toolchain {
        label: "git",
        key: "git",
        bins: &["git"],
        version_arg: "--version",
        why: "missions + forge: clone, commit, push, open PRs",
        required: true,
    },
    Toolchain {
        label: "cargo (Rust)",
        key: "rust",
        bins: &["cargo"],
        version_arg: "--version",
        why: "forge build/test gate on Rust repos (cargo check / cargo test)",
        required: false,
    },
    Toolchain {
        label: "python",
        key: "python",
        bins: &["python", "python3"],
        version_arg: "--version",
        why: "codet syntax/test checks + the pytest forge gate",
        required: false,
    },
    Toolchain {
        label: "node",
        key: "node",
        bins: &["node"],
        version_arg: "--version",
        why: "codet JS/TS checks + the npm forge gate",
        required: false,
    },
    Toolchain {
        label: "go",
        key: "go",
        bins: &["go"],
        version_arg: "version",
        why: "forge build/test gate on Go repos (go build / go test)",
        required: false,
    },
];

fn probe_toolchains() -> Status {
    let mut worst = Status::Ok;
    for tc in TOOLCHAINS {
        match tc
            .bins
            .iter()
            .find_map(|b| command_first_line(b, tc.version_arg))
        {
            Some(version) => print_row(tc.label, Status::Ok, &version),
            None => {
                let detail = format!("not found — needed for: {}", tc.why);
                if tc.required {
                    print_row(tc.label, Status::Err, &detail);
                    worst = Status::Err;
                } else {
                    print_row(tc.label, Status::Warn, &detail);
                    if worst == Status::Ok {
                        worst = Status::Warn;
                    }
                }
                print_fix(&toolchain_install_hint(tc.key));
            }
        }
    }
    worst
}

/// Run `<bin> <arg>` and return its first non-empty output line, or `None` when
/// the binary isn't on PATH / couldn't be executed. Version banners land on
/// stdout for most tools but stderr for a few, so both streams are considered.
fn command_first_line(bin: &str, arg: &str) -> Option<String> {
    let out = Command::new(bin).arg(arg).output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pick = if stdout.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        stdout.into_owned()
    };
    pick.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// OS-appropriate copy-paste install command for a toolchain `key`.
fn toolchain_install_hint(key: &str) -> String {
    let os = std::env::consts::OS;
    let cmd = match (os, key) {
        ("windows", "git") => "winget install Git.Git",
        ("windows", "rust") => "winget install Rustlang.Rustup  (then `rustup default stable`)",
        ("windows", "python") => "winget install Python.Python.3.12",
        ("windows", "node") => "winget install OpenJS.NodeJS.LTS",
        ("windows", "go") => "winget install GoLang.Go",
        ("windows", "ffmpeg") => "winget install Gyan.FFmpeg",
        ("macos", "git") => "brew install git",
        ("macos", "rust") => "brew install rustup && rustup-init",
        ("macos", "python") => "brew install python",
        ("macos", "node") => "brew install node",
        ("macos", "go") => "brew install go",
        ("macos", "ffmpeg") => "brew install ffmpeg",
        (_, "git") => "sudo apt install git   (or your distro's package manager)",
        (_, "rust") => "curl https://sh.rustup.rs -sSf | sh",
        (_, "python") => "sudo apt install python3",
        (_, "node") => "sudo apt install nodejs npm",
        (_, "go") => "sudo apt install golang   (or https://go.dev/dl/)",
        (_, "ffmpeg") => "sudo apt install ffmpeg",
        _ => "see the tool's official install docs",
    };
    format!("install: {cmd}")
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

/// Surface the offline-mode posture and, when enforced, the exact egress
/// allow-list. Purely informational — it never fails the overall run (an
/// air-gapped box is a healthy box, and "offline off" is the opt-in default,
/// not a misconfiguration).
fn probe_egress() -> Status {
    if crate::egress::is_offline() {
        print_row(
            "offline mode",
            Status::Ok,
            "ENFORCED — only the hosts below are reachable",
        );
        for host in crate::egress::allow_list() {
            print_row("  allow", Status::Ok, &host);
        }
        print_row(
            "  deny",
            Status::Ok,
            "everything else (web_search/web_fetch, gmail/calendar, weather/wikipedia, github, telegram)",
        );
    } else {
        print_row(
            "offline mode",
            Status::Ok,
            "off — run with --offline (or CLAUDETTE_OFFLINE=1) to enforce the air-gap",
        );
    }
    Status::Ok
}

#[cfg(feature = "integrations")]
fn probe_google_oauth() -> Status {
    // Offline mode blocks every Google API call by design — attempting the
    // live verify here would just paint the report red. Show it as skipped so
    // the report stays green-meaningful.
    if crate::egress::is_offline() {
        print_row(
            "google oauth",
            Status::Ok,
            "skipped — offline mode blocks Google API access",
        );
        return Status::Ok;
    }
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
#[cfg(feature = "integrations")]
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
        print_fix(&toolchain_install_hint("ffmpeg"));
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
        print_fix(
            "build whisper.cpp (`whisper-cli`) from https://github.com/ggml-org/whisper.cpp, \
             or set CLAUDETTE_WHISPER_BIN to its path",
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

    // ─── Build-toolchain probes + copy-paste fixes ────────────────────

    #[test]
    fn command_first_line_finds_present_binary() {
        // cargo is always on PATH in the test environment.
        let v = command_first_line("cargo", "--version");
        assert!(v.is_some(), "cargo --version should resolve in tests");
        assert!(v.unwrap().to_lowercase().contains("cargo"));
    }

    #[test]
    fn command_first_line_none_for_missing_binary() {
        assert!(command_first_line("claudette-no-such-binary-xyz", "--version").is_none());
    }

    #[test]
    fn toolchain_install_hint_is_nonempty_for_every_key() {
        for key in ["git", "rust", "python", "node", "go", "ffmpeg"] {
            let h = toolchain_install_hint(key);
            assert!(h.starts_with("install: "), "key {key} got: {h}");
            assert!(h.len() > "install: ".len(), "key {key} has no command: {h}");
        }
    }

    #[test]
    fn backend_start_hint_is_backend_specific() {
        assert!(backend_start_hint(false).to_lowercase().contains("ollama"));
        assert!(backend_start_hint(true)
            .to_lowercase()
            .contains("lm studio"));
    }

    #[test]
    fn model_load_hint_ollama_uses_pull_command() {
        assert!(model_load_hint(false, "qwen3:8b").contains("ollama pull qwen3:8b"));
        assert!(model_load_hint(true, "any")
            .to_lowercase()
            .contains("lm studio"));
    }

    #[test]
    fn git_is_the_only_required_toolchain() {
        // Missions/forge can't function without git; the language toolchains
        // are only needed when you forge in that language, so they warn.
        let required: Vec<&str> = TOOLCHAINS
            .iter()
            .filter(|t| t.required)
            .map(|t| t.key)
            .collect();
        assert_eq!(required, vec!["git"]);
    }
}
