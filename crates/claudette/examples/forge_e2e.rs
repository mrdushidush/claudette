//! Tests 12 + 13 + 14 — forge-mode e2e on agent-battle-command-center.
//!
//! Subcommands:
//!   probe              → Test 13: load models.toml + bundled personas, print
//!                        resolution. No live brain.
//!   run [slug]         → Tests 12 + 14: mission_start → run_forge_mission
//!                        → mission_exit. Opens a real PR only when
//!                        CLAUDETTE_REAL_PR=1 (the Submitter brain turn
//!                        invokes `mission_submit` which calls gh_create_pr).
//!                        Slug defaults to `forge_e2e_<unix>` and is written
//!                        to a marker file for the `close` subcommand.
//!   close              → Read the marker file, close the PR opened by the
//!                        last `run`, delete its branch, and remove the
//!                        mission dir. Idempotent.
//!
//! Env requirements for `run`:
//!   CLAUDETTE_MODEL set, LM Studio running with that model loaded.
//!   GITHUB_TOKEN (or CLAUDETTE_GITHUB_TOKEN) set if you want the submit
//!   phase to push + open a PR.
//!
//! Example:
//!   $env:GITHUB_TOKEN = (gh auth token); $env:CLAUDETTE_REAL_PR = "1"
//!   cargo run --example forge_e2e -- run

use claudette::forge::personas::load_personas;
use claudette::forge::types::{ModelMap, Role};
use claudette::tools::dispatch_tool;
use claudette::{run_forge_mission, SessionOptions};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const OWNER: &str = "mrdushidush";
const REPO: &str = "agent-battle-command-center";

fn pretty(out: &str) -> String {
    serde_json::from_str::<Value>(out)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| out.to_string())
}

fn run_tool(name: &str, input: &str) -> Result<String, String> {
    println!("── {name}({input})");
    match dispatch_tool(name, input) {
        Ok(out) => {
            println!("{}\n", pretty(&out));
            Ok(out)
        }
        Err(e) => {
            println!("ERR: {e}\n");
            Err(e)
        }
    }
}

fn marker_path() -> std::path::PathBuf {
    std::env::temp_dir().join("forge_e2e_last_dest.txt")
}

fn probe() {
    println!("=== Test 13: forge config probe ===\n");

    let toml_path = claudette::forge::models_toml::default_toml_path();
    println!("models.toml path  : {}", toml_path.display());
    println!("models.toml exists: {}\n", toml_path.exists());

    match ModelMap::load() {
        Ok(map) => {
            println!("Resolved role models:");
            for role in [
                Role::Assistant,
                Role::Planner,
                Role::Router,
                Role::Coder,
                Role::TestCoder,
                Role::Verifier,
                Role::SurgicalCoder,
                Role::Cto,
            ] {
                if let Some((kind, name)) = map.resolve(role) {
                    println!("  {role:?} = {name:?} via {kind:?}");
                }
            }
        }
        Err(e) => println!("ModelMap::load() ERR: {e}"),
    }
    println!();

    let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("personas");
    println!("Bundled personas dir: {}", bundled.display());
    match load_personas(&bundled, None) {
        Ok(map) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            println!("Loaded personas: {keys:?}");
            if let Some(codex7) = map.get("codex7") {
                println!(
                    "  codex7 → name={:?} role={:?} voice={:?} backstory_chars={} examples={}",
                    codex7.name,
                    codex7.role,
                    codex7.voice,
                    codex7.backstory.len(),
                    codex7.examples.len(),
                );
            }
        }
        Err(e) => println!("load_personas ERR: {e}"),
    }
}

fn run_e2e(slug_arg: Option<String>) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dest = slug_arg.unwrap_or_else(|| format!("forge_e2e_{ts}"));

    if std::env::var("GITHUB_TOKEN").is_err() && std::env::var("CLAUDETTE_GITHUB_TOKEN").is_err() {
        eprintln!(
            "GITHUB_TOKEN not set — mission_start (git_clone of a public repo) will work but \
             the Submitter phase will fail at gh_create_pr. Run:\n  \
             $env:GITHUB_TOKEN = (gh auth token)\nfirst if you want a real PR."
        );
    }

    println!("\n=== Tests 12+14: forge e2e on {OWNER}/{REPO} ===\n");

    if run_tool(
        "mission_start",
        &json!({"target": format!("{OWNER}/{REPO}"), "dest": &dest}).to_string(),
    )
    .is_err()
    {
        std::process::exit(1);
    }

    let _ = std::fs::write(marker_path(), &dest);

    // Tight prompt: force `write_file` (no bash rabbit-holes) and leave the
    // file UNCOMMITTED so mission_submit (in the Submitter phase) handles
    // git_add → git_commit → push → PR. Gemma-4-26b will thrash on shell-
    // encoding issues if given bash freedom on Windows; structured tools are
    // deterministic. The Submitter contract is: Coder leaves a dirty working
    // tree, mission_submit stages+commits+pushes+opens-PR in one tool call.
    let prompt = format!(
        "Task: append the line `forge-e2e {ts}` (exactly that, no surrounding markdown) \
         to the end of README.md.\n\n\
         Procedure — follow exactly:\n\
         1. Call read_file with path README.md to fetch the current content.\n\
         2. Call write_file with path README.md and content = old_content + \"\\n\" + \
         \"forge-e2e {ts}\\n\". Do NOT modify anything else.\n\
         3. Stop after the write_file call succeeds.\n\n\
         Hard constraints:\n\
         - Do NOT use bash, python, powershell, or any shell tool. Use write_file only.\n\
         - Do NOT call git_add, git_commit, git_push, or mission_submit. \
         The Submitter phase will stage + commit + push + open the PR for you.\n\
         - Leave the working tree dirty (modified README.md, no commit). Stop after step 2."
    );

    let opts = SessionOptions {
        resume: false,
        autosave: false,
    };
    match run_forge_mission(&prompt, opts) {
        Ok(summary) => {
            println!(
                "\n✔ forge completed: iter={} in_tok={} out_tok={}",
                summary.iterations, summary.usage.input_tokens, summary.usage.output_tokens,
            );
        }
        Err(e) => {
            println!("\n✘ forge ERR: {e:#}");
        }
    }

    let _ = run_tool("mission_exit", "{}");

    println!(
        "\nMission slug written to: {}\n(Use `cargo run --example forge_e2e -- close` to clean up.)",
        marker_path().display()
    );
}

fn close_e2e() {
    let marker = marker_path();
    let slug = match std::fs::read_to_string(&marker) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no marker at {} — nothing to close", marker.display());
            std::process::exit(0);
        }
    };
    if slug.is_empty() {
        eprintln!("marker at {} was empty", marker.display());
        std::process::exit(0);
    }
    println!("Closing forge_e2e run for slug: {slug}\n");

    // The branch name is auto-generated by mission_submit as
    // `claudette-mission/<slug>`. Use gh to look for an open PR with that
    // head, close it, delete the branch, then rm -rf the mission dir.
    let head = format!("{OWNER}:claudette-mission/{slug}");

    let list = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            &format!("{OWNER}/{REPO}"),
            "--head",
            &format!("claudette-mission/{slug}"),
            "--state",
            "open",
            "--json",
            "number,headRefName",
        ])
        .output();
    if let Ok(out) = list {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            println!("gh pr list ({head}):\n{stdout}");
            let v: Value = serde_json::from_str(&stdout).unwrap_or(Value::Array(vec![]));
            if let Some(arr) = v.as_array() {
                for entry in arr {
                    if let Some(n) = entry.get("number").and_then(Value::as_i64) {
                        let _ = std::process::Command::new("gh")
                            .args([
                                "pr",
                                "close",
                                "--repo",
                                &format!("{OWNER}/{REPO}"),
                                &n.to_string(),
                                "--delete-branch",
                            ])
                            .status();
                        println!("closed PR #{n}");
                    }
                }
            }
        }
    }

    // Best-effort: delete the local mission dir.
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let mission_dir = std::path::Path::new(&home)
        .join(".claudette")
        .join("missions")
        .join(&slug);
    if mission_dir.exists() {
        match std::fs::remove_dir_all(&mission_dir) {
            Ok(()) => println!("removed mission dir {}", mission_dir.display()),
            Err(e) => println!("could not remove {}: {e}", mission_dir.display()),
        }
    }

    let _ = std::fs::remove_file(&marker);
    println!("\ndone.");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "probe".to_string());
    match cmd.as_str() {
        "probe" => probe(),
        "run" => run_e2e(args.next()),
        "close" => close_e2e(),
        other => {
            eprintln!("unknown subcommand: {other} (use: probe, run, close)");
            std::process::exit(2);
        }
    }
}
