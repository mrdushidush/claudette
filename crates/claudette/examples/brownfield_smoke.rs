//! Brownfield smoke test — exercises the 5 tools added for T1 against
//! real GitHub endpoints. Skips `gh_create_pr` (would open a real PR).
//!
//! Run with:
//!   $env:GITHUB_TOKEN = (gh auth token); cargo run --example brownfield_smoke
//!
//! Cleanup hints are printed at the end.

use claudette::tools::dispatch_tool;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

fn show(label: &str, name: &str, input: &str) -> Result<String, String> {
    println!("\n── {label}");
    println!("   {name}({input})");
    let result = dispatch_tool(name, input);
    match &result {
        Ok(out) => {
            // Pretty-print: try to parse as JSON, else show first 600 chars.
            if let Ok(v) = serde_json::from_str::<Value>(out) {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| out.clone());
                println!("   ✓ {pretty}");
            } else {
                println!("   ✓ {out}");
            }
        }
        Err(e) => println!("   ✗ {e}"),
    }
    result
}

fn main() {
    println!("=== claudette brownfield smoke test ===");

    if std::env::var("GITHUB_TOKEN").is_err() && std::env::var("CLAUDETTE_GITHUB_TOKEN").is_err() {
        eprintln!(
            "GITHUB_TOKEN not set. Run:\n  \
             $env:GITHUB_TOKEN = (gh auth token); cargo run --example brownfield_smoke"
        );
        std::process::exit(1);
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let dest = format!("smoketest_{timestamp}");

    // 1. List issues — pure read.
    let _ = show(
        "list issues on mrdushidush/claudette (max 5)",
        "gh_list_repo_issues",
        &json!({
            "owner": "mrdushidush",
            "repo": "claudette",
            "limit": 5,
        })
        .to_string(),
    );

    // 2. PR status — pure read against a recent merged PR.
    let _ = show(
        "PR status: rust-lang/rust#156324",
        "gh_pr_status",
        &json!({
            "owner": "rust-lang",
            "repo": "rust",
            "number": 156324,
        })
        .to_string(),
    );

    // 3. Clone — writes to ~/.claudette/missions/<dest>/.
    let clone_result = show(
        &format!("clone octocat/Hello-World → ~/.claudette/missions/{dest}"),
        "git_clone",
        &json!({
            "url": "https://github.com/octocat/Hello-World.git",
            "dest": dest,
            "depth": 1,
        })
        .to_string(),
    );

    // 4. Fork — creates mrdushidush/Hello-World on GitHub.
    let _ = show(
        "fork octocat/Hello-World",
        "gh_fork",
        &json!({
            "owner": "octocat",
            "repo": "Hello-World",
        })
        .to_string(),
    );

    println!("\n=== smoke test done ===");
    println!("\nCleanup if you want:");
    if clone_result.is_ok() {
        println!("  Remove-Item -Recurse -Force ~/.claudette/missions/{dest}");
    }
    println!("  gh repo delete mrdushidush/Hello-World --yes  (deletes the fork)");
}
