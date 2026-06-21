//! Proves the air-gap end-to-end.
//!
//! v0.8.9 shipped enforced offline mode, but until now it was tested only at
//! the *policy-function* level (`egress::is_allowed_host`). Nothing proved that
//! the actual tool-dispatch path — the thing the model drives — makes zero
//! cloud connections under `--offline`. This integration test closes that gap:
//! with `CLAUDETTE_OFFLINE=1`, it drives every network-reaching tool through
//! the real [`dispatch_tool`] entry point and asserts each one refuses with the
//! uniform [`egress::BLOCK_PREFIX`] message *before* any request leaves the
//! process. The air-gap is now a CI-proven guarantee, not just a posture.
//!
//! Approach: dispatch-level assertion (no sockets). Every guard fires before
//! the tool opens a connection, so a refusal at the dispatch layer is proof of
//! zero egress. We point `OLLAMA_HOST` at loopback so the backend stays
//! allow-listed (the brain/recall path must keep working under offline mode).

use claudette::agent_tools_json;
use claudette::egress::{self, BLOCK_PREFIX, INTEGRATION_NET_TOOLS, NET_TOOLS};
use claudette::tools::dispatch_tool;

/// Every network-reaching tool present in *this* build: the always-on set plus
/// the integration-only set (empty in a coding-only build). The air-gap proof
/// must cover exactly the tools the schema actually advertises.
fn all_net_tools() -> impl Iterator<Item = &'static str> {
    NET_TOOLS.iter().chain(INTEGRATION_NET_TOOLS).copied()
}

/// Serialises the env-mutating tests: `CLAUDETTE_OFFLINE` / `OLLAMA_HOST` are
/// process-global, so parallel tests in this binary would otherwise race.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Enables offline mode + a loopback backend for a test's duration and restores
/// the prior environment on drop (mirrors the env-guard in `egress.rs`'s unit
/// tests). Declare it *after* the `ENV_LOCK` guard so it drops first — env is
/// restored while the lock is still held.
struct OfflineEnv {
    offline: Option<String>,
    ollama: Option<String>,
}

impl OfflineEnv {
    fn set() -> Self {
        let prev = Self {
            offline: std::env::var(egress::OFFLINE_ENV).ok(),
            ollama: std::env::var("OLLAMA_HOST").ok(),
        };
        std::env::set_var(egress::OFFLINE_ENV, "1");
        std::env::set_var("OLLAMA_HOST", "http://localhost:11434");
        prev
    }
}

impl Drop for OfflineEnv {
    fn drop(&mut self) {
        restore(egress::OFFLINE_ENV, self.offline.as_deref());
        restore("OLLAMA_HOST", self.ollama.as_deref());
    }
}

fn restore(key: &str, val: Option<&str>) {
    match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

/// The minimal input each tool needs to *reach* its egress guard. Most tools
/// guard at the dispatch/group level (or the first line of their handler)
/// before touching their input, so `{}` is enough. Three parse input first:
/// `web_fetch` needs a public URL that clears the SSRF check, and
/// `git_clone` / `mission_start` need a remote target — each then hits the
/// guard before any side effect (network *or* filesystem).
fn offline_probe_input(tool: &str) -> &'static str {
    match tool {
        "web_fetch" => r#"{"url":"https://1.1.1.1/"}"#,
        "git_clone" => {
            r#"{"url":"https://github.com/octocat/Hello-World.git","dest":"claudette-offline-probe"}"#
        }
        "mission_start" => r#"{"target":"octocat/Hello-World"}"#,
        _ => "{}",
    }
}

#[test]
fn every_net_tool_refuses_under_offline() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _env = OfflineEnv::set();

    for tool in all_net_tools() {
        match dispatch_tool(tool, offline_probe_input(tool)) {
            Ok(out) => panic!("{tool} reached the network under offline mode (returned Ok): {out}"),
            Err(err) => assert!(
                err.starts_with(BLOCK_PREFIX),
                "{tool} must refuse with the uniform air-gap message, got: {err}"
            ),
        }
    }
}

#[test]
fn backend_and_loopback_stay_allowed_under_offline() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _env = OfflineEnv::set();

    // The backend / recall-embeddings path is on the allow-list — offline mode
    // must NOT block it, or the local model itself would stop working. This is
    // the positive control for the blocks above.
    assert!(
        egress::guard("http://localhost:11434/api/embeddings").is_ok(),
        "loopback backend must stay reachable under offline mode"
    );
    assert!(
        egress::guard("http://127.0.0.1:1234/v1/chat/completions").is_ok(),
        "LM Studio loopback must stay reachable under offline mode"
    );
}

#[test]
fn net_tools_registry_is_consistent_and_covers_network_families() {
    use std::collections::HashSet;

    // (a) No duplicate entries in the registry (across both lists).
    let mut seen = HashSet::new();
    for t in all_net_tools() {
        assert!(seen.insert(t), "duplicate entry in NET_TOOLS: {t}");
    }

    // (b) Every registered net tool is a real, dispatchable tool that appears
    // in the advertised schema (catches a rename/removal that would silently
    // drop a tool from coverage).
    let schema = agent_tools_json();
    let schema_names: HashSet<String> = schema
        .as_array()
        .expect("tool schema is a JSON array")
        .iter()
        .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect();
    for t in all_net_tools() {
        assert!(
            schema_names.contains(t),
            "NET_TOOLS lists '{t}' but it is not in the tool schema (renamed or removed?)"
        );
    }

    // (c) Forgotten-guard regression guard: every tool in an *always-network*
    // family must be listed. Add a new `gh_*` (etc.) tool but forget the guard
    // + registry entry and this fails loudly. `git_*` / `mission_*` have local
    // siblings, so they can't be swept by prefix and are maintained by hand in
    // egress::NET_TOOLS.
    const ALWAYS_NET_PREFIXES: &[&str] = &["gh_", "gmail_", "calendar_", "tg_"];
    let registry: HashSet<&str> = all_net_tools().collect();
    for name in &schema_names {
        if ALWAYS_NET_PREFIXES.iter().any(|p| name.starts_with(p)) {
            assert!(
                registry.contains(name.as_str()),
                "tool '{name}' is in an always-network family but missing from \
                 egress::NET_TOOLS — add it there and wire its egress::guard call"
            );
        }
    }
}
