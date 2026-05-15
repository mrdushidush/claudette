//! Test 6 smoke — exercise the existing Google OAuth tokens without going
//! through the brain. Calls `calendar_list_events` (ReadOnly, auto-allowed
//! by the perm policy) and `gmail_list` (also ReadOnly) directly via the
//! dispatcher. Success means refresh-token + token-exchange still work end
//! to end; failure surfaces as a clear error string.
//!
//! Run: cargo run --example oauth_smoke

use claudette::tools::dispatch_tool;
use serde_json::Value;

fn call(name: &str, input: &str) {
    println!("── {name}({input})");
    match dispatch_tool(name, input) {
        Ok(out) => {
            let pretty = serde_json::from_str::<Value>(&out)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or(out);
            // Truncate huge payloads to keep output readable.
            let display = if pretty.len() > 1500 {
                format!(
                    "{}\n... ({} bytes total)",
                    &pretty[..1500.min(pretty.len())],
                    pretty.len()
                )
            } else {
                pretty
            };
            println!("OK\n{display}\n");
        }
        Err(e) => println!("ERR: {e}\n"),
    }
}

fn main() {
    // .env loading lives in `claudette::main`, not the library. Examples
    // need to opt in manually so CLAUDETTE_GOOGLE_CLIENT_ID + _SECRET come
    // through from `~/.claudette/.env`.
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let env_path = std::path::Path::new(&home).join(".claudette").join(".env");
        let _ = dotenvy::from_path(&env_path);
    }

    println!("=== Test 6: Google OAuth tokens — live read-only smoke ===\n");

    // Calendar read — should succeed via google_oauth.json's refresh token.
    call("calendar_list_events", r#"{"max_results":3}"#);

    // Gmail read — should succeed via google_oauth_gmail_read.json's refresh token.
    call("gmail_list", r#"{"max_results":3}"#);
}
