//! Offline mode — the *enforced* backing of claudette's "air-gapped by design"
//! claim. Until now "air-gapped" was a posture: claudette only talks to a local
//! model by default, but nothing stopped a tool (or a prompt-injected model)
//! from reaching the open internet. Offline mode turns that posture into a
//! guarantee.
//!
//! When enabled — `--offline` on the CLI, or `CLAUDETTE_OFFLINE=1` in the
//! environment — every outbound network call is checked against a tiny
//! allow-list: the configured local model backend (the resolved Ollama /
//! LM Studio host, even if it's a LAN box you own) plus loopback. Anything
//! else is hard-blocked with a single, uniform message so the refusal reads
//! identically no matter which code path tripped it.
//!
//! Two enforcement layers, because not all egress is in-process:
//!
//!  1. **HTTP layer** — [`guard`] is called on the resolved destination host
//!     in the reqwest path of each network-reaching tool, before the request
//!     leaves the process. Recall embeddings and brain / vision calls to the
//!     local backend pass the allow-list; `web_search` / `web_fetch` / `gmail`
//!     / `calendar` / `google_auth` / `wikipedia` / `weather`
//!     / Telegram are blocked.
//!
//!  2. **Dispatch / subprocess layer** — tools that shell out to the network
//!     instead of using reqwest can't be seen by the HTTP layer, so they call
//!     [`guard_subprocess`] up front and refuse with the SAME message: the
//!     `git_push` / `git_clone` subprocesses, the brownfield `mission_start`
//!     clone and `mission_submit` push, and the edge-tts TTS subprocess.
//!
//! The allow / deny decision is pure host-matching ([`is_allowed_host`]); the
//! flag/env plumbing ([`is_offline`]) and both message builders are unit-tested
//! below.

use crate::api::{host_of_url, is_local_ollama_url, resolve_ollama_url};

/// Environment variable that enables offline mode. The `--offline` CLI flag
/// sets this to `"1"` (see `main.rs`) so the flag and the env var share one
/// source of truth — and so the setting propagates to any child process
/// claudette spawns (`gh`, `git`, `python -m edge_tts`, …), which then refuse
/// network access themselves if they re-enter claudette.
pub const OFFLINE_ENV: &str = "CLAUDETTE_OFFLINE";

/// Shared prefix on every offline-block message. Centralised so the HTTP-layer
/// refusal and the subprocess-layer refusal are byte-for-byte recognisable as
/// "the same block", and so tests can assert on it without pinning the whole
/// sentence.
pub const BLOCK_PREFIX: &str = "blocked by offline mode (--offline / CLAUDETTE_OFFLINE)";

/// Canonical registry of every tool whose normal operation reaches the network
/// and is therefore gated under offline mode — plus the raw-shell escape hatch
/// (`bash` / `bash_background`) and the build/test toolchain runners
/// (`run_tests` / `diagnostics`), which are *not* network-by-default but are
/// refused wholesale because their egress is unguardable (arbitrary shell /
/// build-script / test-code execution). All are gated by [`guard`] /
/// [`guard_subprocess`] / the bash + toolchain offline-refusals. This is the
/// single source of truth the no-egress integration test
/// (`tests/offline_egress.rs`) iterates: it drives each tool through
/// `dispatch_tool` with `CLAUDETTE_OFFLINE=1` and asserts every one refuses
/// with a [`BLOCK_PREFIX`] message — turning the air-gap from a documented
/// posture into a CI-proven guarantee.
///
/// MAINTENANCE CONTRACT: when you add a tool that performs network egress, add
/// its name here *and* wire its `egress::guard*` call. The integration test
/// fails if a tool listed here is not actually guarded; the registry test
/// (`net_tools_registry_covers_network_named_tools`) fails if a tool in an
/// always-network family (`gh_`, `gmail_`, `calendar_`, `tg_`) is missing from
/// this list — so a forgotten guard on those families is caught automatically.
/// Tools in mixed families (`git_*`, `mission_*` have local siblings) must be
/// added here by hand.
pub const NET_TOOLS: &[&str] = &[
    // Search / fetch
    "web_search",
    "web_fetch",
    // GitHub (REST via reqwest → api.github.com)
    "gh_inbox",
    "gh_get_issue",
    "gh_create_issue",
    "gh_comment_issue",
    "gh_search_code",
    "gh_list_repo_issues",
    "gh_pr_status",
    "gh_pr_view",
    "gh_workflow_logs",
    "gh_fork",
    "gh_create_pr",
    // Keyless facts
    "wikipedia",
    "weather",
    // Google (Gmail + Calendar) + Telegram are integration-only and listed in
    // INTEGRATION_NET_TOOLS below, so this list stays consistent with the
    // coding-only schema (where those tools don't exist).
    // Network-reaching git + brownfield-mission subprocesses
    "git_push",
    "git_clone",
    "mission_start",
    "mission_submit",
    // Raw-shell escape hatch. `bash` / `bash_background` run an arbitrary
    // command, so they are an unguardable egress vector (a curl/scp/python
    // denylist leaks by construction). Under offline mode they are refused
    // *wholesale* — the honest posture — so they belong in the air-gap proof.
    // (roast 2026-06-21, Wave 1.1)
    "bash",
    "bash_background",
    // Build/test toolchain runners. `run_tests` / `diagnostics` shell out to
    // cargo / npm / pytest / go, which compile and execute arbitrary build
    // scripts, test bodies, and proc-macros and may fetch uncached dependencies
    // from a package registry — the SAME unguardable egress vector as `bash`
    // (the guard cannot inspect what a build script does). Refused wholesale
    // under offline mode. (roast 2026-06-30, H1)
    "run_tests",
    "diagnostics",
];

/// Network-reaching tools that exist **only** in an `integrations` build:
/// Gmail/Calendar (Google REST) and the Telegram bridge. Kept separate from
/// [`NET_TOOLS`] so the air-gap proof stays consistent with the advertised
/// schema in a coding-only build, where these tools are stubbed out entirely.
/// Empty when the feature is off. The offline-egress test iterates both lists.
#[cfg(feature = "integrations")]
pub const INTEGRATION_NET_TOOLS: &[&str] = &[
    "gmail_list",
    "gmail_search",
    "gmail_read",
    "gmail_list_labels",
    "calendar_list_events",
    "calendar_create_event",
    "calendar_update_event",
    "calendar_delete_event",
    "tg_send",
];
/// Coding-only build: no Gmail/Calendar/Telegram tools are compiled in.
#[cfg(not(feature = "integrations"))]
pub const INTEGRATION_NET_TOOLS: &[&str] = &[];

/// Returns true when offline mode is enabled. Truthy = set, non-empty, and not
/// literally `"0"` — matching the convention used by `CLAUDETTE_FACELESS`,
/// `CLAUDETTE_ALLOW_REMOTE_OLLAMA`, and the skip-probe flags.
#[must_use]
pub fn is_offline() -> bool {
    std::env::var(OFFLINE_ENV)
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
}

/// The allow-list, as host strings, for display in `--doctor`. Order: loopback
/// names first, then the resolved backend host (de-duplicated if the backend
/// is itself loopback, which is the common case).
#[must_use]
pub fn allow_list() -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.0/8".to_string(),
        "::1".to_string(),
    ];
    let backend = host_of_url(&resolve_ollama_url());
    if !backend.is_empty() && !is_loopback_host(&backend) {
        hosts.push(backend);
    }
    hosts
}

/// True when `host` (already a bare lowercased host, as from [`host_of_url`])
/// is a loopback address. Thin wrapper over [`is_local_ollama_url`] so callers
/// holding a host rather than a URL don't have to re-synthesise a URL.
#[must_use]
fn is_loopback_host(host: &str) -> bool {
    // `is_local_ollama_url` parses a URL; feed it a bare host (no scheme),
    // which `host_of_url` passes through unchanged.
    is_local_ollama_url(host)
}

/// Core policy: is a request to `url` permitted under offline mode? Pure,
/// side-effect-free, and independent of whether offline mode is actually on —
/// so it can be unit-tested directly. Allowed iff the host is loopback OR the
/// configured backend host.
///
/// The backend host is matched at the *host* level, not host+port: a LAN
/// backend like `http://192.168.1.50:11434` means "that box is my hardware",
/// so other ports on the same box are allowed too. This is intentional and
/// documented (`CLAUDETTE_ALLOW_REMOTE_OLLAMA` is the knob that legitimised a
/// LAN backend in the first place).
#[must_use]
pub fn is_allowed_host(url: &str) -> bool {
    if is_local_ollama_url(url) {
        return true;
    }
    let host = host_of_url(url);
    if host.is_empty() {
        return false;
    }
    let backend = host_of_url(&resolve_ollama_url());
    !backend.is_empty() && host.eq_ignore_ascii_case(&backend)
}

/// HTTP-layer guard. Call with the destination URL (or bare host) immediately
/// before a reqwest `.send()`. A no-op when offline mode is off; otherwise
/// returns `Ok(())` for allow-listed hosts and a [`BLOCK_PREFIX`]-prefixed
/// `Err` for everything else.
///
/// # Errors
/// Returns the uniform offline-block message when offline mode is on and `url`
/// is not on the allow-list.
pub fn guard(url: &str) -> Result<(), String> {
    if !is_offline() || is_allowed_host(url) {
        return Ok(());
    }
    let host = host_of_url(url);
    let host = if host.is_empty() { url } else { &host };
    Err(format!(
        "{BLOCK_PREFIX}: outbound connection to '{host}' is not allowed. Only the local \
         model backend ({}) and loopback are reachable. Disable offline mode to use this.",
        resolve_ollama_url()
    ))
}

/// Dispatch / subprocess-layer guard. Call at the top of a tool that reaches
/// the network by spawning a subprocess (`git`, `gh`, `python -m edge_tts`)
/// rather than through reqwest, where [`guard`] can't see the destination.
/// `action` is a short human phrase naming what was attempted, e.g.
/// `"git_push (push to the remote repository)"`.
///
/// A no-op when offline mode is off.
///
/// # Errors
/// Returns the uniform offline-block message when offline mode is on.
pub fn guard_subprocess(action: &str) -> Result<(), String> {
    if !is_offline() {
        return Ok(());
    }
    Err(format!(
        "{BLOCK_PREFIX}: {action} requires network access, which is disabled. Only the local \
         model backend ({}) and loopback are reachable. Disable offline mode to use this.",
        resolve_ollama_url()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialise env-mutating tests: `OLLAMA_HOST` / `CLAUDETTE_OFFLINE` are
    /// process-global, so parallel tests would race. Each test takes this lock
    /// and restores the prior values on the way out.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        offline: Option<String>,
        ollama: Option<String>,
    }
    impl EnvGuard {
        fn capture() -> Self {
            Self {
                offline: std::env::var(OFFLINE_ENV).ok(),
                ollama: std::env::var("OLLAMA_HOST").ok(),
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore(OFFLINE_ENV, self.offline.as_deref());
            restore("OLLAMA_HOST", self.ollama.as_deref());
        }
    }
    fn restore(key: &str, val: Option<&str>) {
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn is_offline_reads_truthy_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();

        std::env::remove_var(OFFLINE_ENV);
        assert!(!is_offline(), "unset → off");

        std::env::set_var(OFFLINE_ENV, "");
        assert!(!is_offline(), "empty → off");

        std::env::set_var(OFFLINE_ENV, "0");
        assert!(!is_offline(), "literal 0 → off");

        std::env::set_var(OFFLINE_ENV, "1");
        assert!(is_offline(), "1 → on");

        std::env::set_var(OFFLINE_ENV, "true");
        assert!(is_offline(), "any other non-empty → on");
    }

    #[test]
    fn loopback_hosts_always_allowed() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        std::env::set_var("OLLAMA_HOST", "http://localhost:11434");

        for url in [
            "http://localhost:11434/api/chat",
            "http://127.0.0.1:1234/v1/chat/completions",
            "http://127.255.255.255:11434",
            "https://[::1]:443/x",
            "localhost:11434",
        ] {
            assert!(is_allowed_host(url), "{url} should be allowed (loopback)");
        }
    }

    #[test]
    fn cloud_hosts_denied() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        std::env::set_var("OLLAMA_HOST", "http://localhost:11434");

        for url in [
            "https://api.github.com/user",
            "https://gmail.googleapis.com/gmail/v1/users/me/messages",
            "https://www.googleapis.com/calendar/v3/calendars/primary/events",
            "https://oauth2.googleapis.com/token",
            "https://api.search.brave.com/res/v1/web/search",
            "https://en.wikipedia.org/w/api.php",
            "https://api.open-meteo.com/v1/forecast",
            "https://api.telegram.org/bot123/getUpdates",
            "https://localhost.evil.com/x",
            "http://user:pass@evil.com/path",
        ] {
            assert!(!is_allowed_host(url), "{url} should be denied (cloud)");
        }
    }

    #[test]
    fn configured_lan_backend_allowed_other_cloud_still_denied() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        // A remote-but-LAN backend the user opted into — "your hardware".
        std::env::set_var("OLLAMA_HOST", "http://192.168.1.50:11434");

        assert!(
            is_allowed_host("http://192.168.1.50:11434/api/chat"),
            "the configured backend host is allowed"
        );
        // Same box, different port — still your hardware, host-level match.
        assert!(
            is_allowed_host("http://192.168.1.50:8080/anything"),
            "other ports on the backend box are allowed (host-level match)"
        );
        // A different LAN box is NOT the backend → denied.
        assert!(
            !is_allowed_host("http://192.168.1.99:11434/api/chat"),
            "a different LAN host is not the backend"
        );
        // Cloud is still denied even with a LAN backend.
        assert!(!is_allowed_host("https://api.github.com/user"));
    }

    #[test]
    fn guard_is_noop_when_offline_off() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        std::env::remove_var(OFFLINE_ENV);
        // Even an obvious cloud host passes when offline mode is off.
        assert!(guard("https://api.github.com/user").is_ok());
        assert!(guard_subprocess("git_push").is_ok());
    }

    #[test]
    fn guard_blocks_cloud_and_allows_backend_when_offline_on() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        std::env::set_var("OLLAMA_HOST", "http://localhost:11434");
        std::env::set_var(OFFLINE_ENV, "1");

        // Backend / loopback pass — recall embeddings stay allowed.
        assert!(guard("http://localhost:11434/api/embeddings").is_ok());

        // Cloud is blocked with the uniform, recognisable message.
        let err = guard("https://api.github.com/user").unwrap_err();
        assert!(
            err.starts_with(BLOCK_PREFIX),
            "uses the shared prefix: {err}"
        );
        assert!(
            err.contains("api.github.com"),
            "names the blocked host: {err}"
        );

        // Subprocess refusal shares the same prefix.
        let sub = guard_subprocess("git_clone (clone a remote repository)").unwrap_err();
        assert!(
            sub.starts_with(BLOCK_PREFIX),
            "subprocess shares the prefix: {sub}"
        );
        assert!(sub.contains("git_clone"), "names the action: {sub}");
    }

    #[test]
    fn allow_list_includes_loopback_and_lan_backend() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();

        std::env::set_var("OLLAMA_HOST", "http://localhost:11434");
        let local = allow_list();
        assert!(local.iter().any(|h| h == "localhost"));
        assert!(local.iter().any(|h| h == "127.0.0.0/8"));
        // Loopback backend isn't repeated as a separate host.
        assert!(!local.iter().any(|h| h == "11434"));

        std::env::set_var("OLLAMA_HOST", "http://192.168.1.50:11434");
        let lan = allow_list();
        assert!(
            lan.iter().any(|h| h == "192.168.1.50"),
            "LAN backend host listed: {lan:?}"
        );
    }
}
