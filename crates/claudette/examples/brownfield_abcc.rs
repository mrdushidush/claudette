//! Live brownfield exploration on agent-battle-command-center.
//!
//! T1 subcommands (each invokes a single brownfield primitive):
//!   list                          → gh_list_repo_issues (default if no args)
//!   body <n>                      → gh_get_issue
//!   clone <dest>                  → git_clone the repo into ~/.claudette/missions/<dest>/
//!   pr <head> <base> <title> <body>  → gh_create_pr on the same repo
//!   status <number>               → gh_pr_status
//!
//! T2 subcommands (mission-driven — exercise the cwd-routed flow):
//!   mission-start <dest>          → mission_start, then git_status from inside it
//!   mission-status                → mission_status
//!   mission-list                  → mission_list
//!   mission-exit                  → mission_exit
//!   mission-submit <title> [body] → capstone (real PR; requires CLAUDETTE_REAL_PR=1)
//!
//! Run with: $env:GITHUB_TOKEN = (gh auth token); cargo run --example brownfield_abcc -- <subcommand> [args]

use claudette::tools::dispatch_tool;
use serde_json::{json, Value};

const OWNER: &str = "mrdushidush";
const REPO: &str = "agent-battle-command-center";
const REPO_URL: &str = "https://github.com/mrdushidush/agent-battle-command-center.git";

fn pretty(out: &str) -> String {
    serde_json::from_str::<Value>(out)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| out.to_string())
}

fn run(name: &str, input: &str) {
    println!("── {name}({input})\n");
    match dispatch_tool(name, input) {
        Ok(out) => println!("{}\n", pretty(&out)),
        Err(e) => {
            println!("ERR: {e}\n");
            std::process::exit(1);
        }
    }
}

/// Subcommands that hit the GitHub REST API and therefore need a token.
/// `mission-start`/`status`/`list`/`exit` and `clone` are pure git ops over
/// HTTPS — public clone, no auth needed.
const NEEDS_TOKEN: &[&str] = &["list", "body", "pr", "status", "mission-submit"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("list", String::as_str);

    if NEEDS_TOKEN.contains(&cmd)
        && std::env::var("GITHUB_TOKEN").is_err()
        && std::env::var("CLAUDETTE_GITHUB_TOKEN").is_err()
    {
        eprintln!(
            "GITHUB_TOKEN not set — `{cmd}` hits the GitHub API. \
             Run: $env:GITHUB_TOKEN = (gh auth token)"
        );
        std::process::exit(1);
    }

    match cmd {
        "list" => {
            println!("=== open issues on {OWNER}/{REPO} (max 30) ===\n");
            let req = json!({"owner": OWNER, "repo": REPO, "limit": 30}).to_string();
            match dispatch_tool("gh_list_repo_issues", &req) {
                Ok(out) => {
                    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
                    let count = v.get("count").and_then(Value::as_u64).unwrap_or(0);
                    println!("count: {count}\n");
                    if let Some(items) = v.get("items").and_then(Value::as_array) {
                        for item in items {
                            let n = item.get("number").and_then(Value::as_i64).unwrap_or(0);
                            let title = item.get("title").and_then(Value::as_str).unwrap_or("");
                            println!("  #{n:<5} {title}");
                        }
                    }
                }
                Err(e) => println!("ERR: {e}"),
            }
        }
        "body" => {
            let number: i64 = args.get(1).and_then(|s| s.parse().ok()).expect("body <n>");
            run(
                "gh_get_issue",
                &json!({"owner": OWNER, "repo": REPO, "number": number}).to_string(),
            );
        }
        "clone" => {
            let dest = args.get(1).expect("clone <dest>");
            run(
                "git_clone",
                &json!({"url": REPO_URL, "dest": dest}).to_string(),
            );
        }
        "pr" => {
            let head = args.get(1).expect("pr <head> <base> <title> <body>");
            let base = args.get(2).expect("pr <head> <base> <title> <body>");
            let title = args.get(3).expect("pr <head> <base> <title> <body>");
            let body = args.get(4).cloned().unwrap_or_default();
            run(
                "gh_create_pr",
                &json!({
                    "owner": OWNER,
                    "repo": REPO,
                    "title": title,
                    "body": body,
                    "head": head,
                    "base": base,
                })
                .to_string(),
            );
        }
        "status" => {
            let number: i64 = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .expect("status <n>");
            run(
                "gh_pr_status",
                &json!({"owner": OWNER, "repo": REPO, "number": number}).to_string(),
            );
        }
        "mission-start" => {
            let dest = args.get(1).expect("mission-start <dest>");
            // Use the bare owner/repo form so mission_submit later knows
            // the canonical GitHub repo without re-parsing the URL.
            run(
                "mission_start",
                &json!({"target": format!("{OWNER}/{REPO}"), "dest": dest}).to_string(),
            );
            // Prove the cwd-routing primitive is live: git_status now
            // reads the just-cloned tree, not claudette's launch dir.
            run("git_status", "{}");
        }
        "mission-status" => run("mission_status", "{}"),
        "mission-list" => run("mission_list", "{}"),
        "mission-exit" => run("mission_exit", "{}"),
        "mission-submit" => {
            // Capstone is destructive (opens a real PR). Gate behind an
            // explicit env var so a stray smoke run doesn't add another
            // open PR to the upstream repo on top of the v0.4.0 #177 leftover.
            if std::env::var("CLAUDETTE_REAL_PR").ok().as_deref() != Some("1") {
                eprintln!(
                    "refusing to call mission_submit without CLAUDETTE_REAL_PR=1 — \
                     this opens a real PR. Set the env var if that's what you want."
                );
                std::process::exit(2);
            }
            let title = args.get(1).expect("mission-submit <title> [body]");
            let body = args.get(2).cloned().unwrap_or_default();
            run(
                "mission_submit",
                &json!({"title": title, "body": body}).to_string(),
            );
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            std::process::exit(2);
        }
    }
}
