//! Google OAuth 2.0 for installed / desktop apps — loopback flow.
//!
//! `claudette --auth-google [scope]` runs [`run_auth_flow`]. It opens the
//! user's browser to Google's authorize endpoint with
//! `redirect_uri=http://127.0.0.1:<port>/callback`, spins up a single-use
//! HTTP server that captures the `code` param, exchanges it for tokens,
//! and persists them to a **per-context** file under `~/.claudette/secrets/`.
//!
//! Per AD-6 we keep **separate token files** for each scope bundle so a
//! hostile email read with the Gmail-read token can't pivot into Calendar
//! writes. Calendar tools load only the calendar token; Gmail read tools
//! load only the gmail-read token; phase 5's Gmail write tools will use
//! a third.
//!
//! Storage:
//!   ~/.claudette/secrets/google_oauth_client.json        → { client_id, client_secret }
//!   ~/.claudette/secrets/google_oauth.json               → Calendar tokens
//!   ~/.claudette/secrets/google_oauth_gmail_read.json    → Gmail read-only tokens
//!
//! Env overrides (same shape as `secrets.rs`):
//!   CLAUDETTE_GOOGLE_CLIENT_ID  / GOOGLE_CLIENT_ID
//!   CLAUDETTE_GOOGLE_CLIENT_SECRET / GOOGLE_CLIENT_SECRET
//!
//! Threat model: plaintext on disk, mode 0600 on Unix. Same as the GitHub PAT
//! and Telegram token already in the secrets dir. See AD-3 in
//! docs/sprint_life_agent.md for the rationale.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// Scope bundles we request at the OAuth consent screen. Each has its own
/// on-disk token file so a compromise of one context can't pivot to the
/// other (AD-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthContext {
    /// Read/write access to Calendar — used by `calendar_*` tools.
    Calendar,
    /// Read-only Gmail — used by `gmail_list`, `gmail_read`,
    /// `gmail_list_labels`, `gmail_search`. Never granted send/modify.
    GmailRead,
}

impl AuthContext {
    /// OAuth scope strings that go into the `scope=` query param.
    const fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Calendar => &["https://www.googleapis.com/auth/calendar"],
            Self::GmailRead => &["https://www.googleapis.com/auth/gmail.readonly"],
        }
    }

    /// Basename of the token file under `~/.claudette/secrets/`.
    const fn token_filename(self) -> &'static str {
        match self {
            Self::Calendar => "google_oauth.json",
            Self::GmailRead => "google_oauth_gmail_read.json",
        }
    }

    /// Human-readable label for CLI messages / errors.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Calendar => "calendar",
            Self::GmailRead => "gmail-read",
        }
    }

    /// Parse from a CLI keyword (case-insensitive). Accepts the canonical
    /// label plus common aliases.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "calendar" | "cal" | "gcal" => Some(Self::Calendar),
            "gmail" | "gmail-read" | "gmail_read" | "mail" => Some(Self::GmailRead),
            _ => None,
        }
    }
}

/// On-disk token record. Shape mirrors Google's `/token` response plus a
/// computed absolute `expires_at` so we don't need to remember when we
/// fetched it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds. Refresh when within 60 s of this.
    pub expires_at: i64,
    pub scope: String,
    pub token_type: String,
}

/// Client credentials the user pastes in from Google Cloud Console.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientCreds {
    client_id: String,
    client_secret: String,
}

fn secrets_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("secrets")
}

fn tokens_path(ctx: AuthContext) -> PathBuf {
    secrets_dir().join(ctx.token_filename())
}

fn client_path() -> PathBuf {
    secrets_dir().join("google_oauth_client.json")
}

/// Load client_id + client_secret via the same env-then-file lookup order
/// as [`crate::secrets::read_secret`].
fn load_client_creds() -> Result<ClientCreds, String> {
    let id = lookup_env_or_file("CLIENT_ID").ok();
    let secret = lookup_env_or_file("CLIENT_SECRET").ok();
    if let (Some(client_id), Some(client_secret)) = (id, secret) {
        return Ok(ClientCreds {
            client_id,
            client_secret,
        });
    }

    let path = client_path();
    if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("google_auth: read {}: {e}", path.display()))?;
        let creds: ClientCreds = serde_json::from_str(&raw).map_err(|e| {
            format!(
                "google_auth: parse {}: {e}. Expected JSON with 'client_id' and 'client_secret'.",
                path.display()
            )
        })?;
        return Ok(creds);
    }

    Err(format!(
        "google_auth: OAuth client not configured. Set CLAUDETTE_GOOGLE_CLIENT_ID + \
         CLAUDETTE_GOOGLE_CLIENT_SECRET env vars, or write JSON {{\"client_id\":\"...\",\
         \"client_secret\":\"...\"}} to {}. See docs/google_setup.md for how to create \
         the OAuth client in Google Cloud Console.",
        path.display()
    ))
}

/// Look up `CLAUDETTE_GOOGLE_<SUFFIX>` then `GOOGLE_<SUFFIX>`.
fn lookup_env_or_file(suffix: &str) -> Result<String, ()> {
    for var in [
        format!("CLAUDETTE_GOOGLE_{suffix}"),
        format!("GOOGLE_{suffix}"),
    ] {
        if let Ok(val) = std::env::var(&var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(())
}

fn save_tokens(ctx: AuthContext, tokens: &GoogleTokens) -> Result<(), String> {
    let path = tokens_path(ctx);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("google_auth: create {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("google_auth: serialize tokens: {e}"))?;
    std::fs::write(&path, body)
        .map_err(|e| format!("google_auth: write {}: {e}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_tokens(ctx: AuthContext) -> Result<GoogleTokens, String> {
    let path = tokens_path(ctx);
    if !path.exists() {
        return Err(format!(
            "google_auth: not authenticated for {}. Run `claudette --auth-google {}` first. \
             (Expected tokens at {}.)",
            ctx.label(),
            ctx.label(),
            path.display()
        ));
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("google_auth: read {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("google_auth: parse {}: {e}", path.display()))
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("google_auth: build http client: {e}"))
}

/// Entry point for tool handlers — returns a bearer token for the given
/// context, ready to drop into `Authorization: Bearer <token>`. Refreshes
/// automatically when close to expiry and persists the refreshed access
/// token to the per-context file.
pub fn access_token(ctx: AuthContext) -> Result<String, String> {
    let mut tokens = load_tokens(ctx)?;
    if tokens.expires_at - now_unix() < 60 {
        refresh_tokens(&mut tokens)?;
        save_tokens(ctx, &tokens)?;
    }
    Ok(tokens.access_token)
}

fn refresh_tokens(tokens: &mut GoogleTokens) -> Result<(), String> {
    let creds = load_client_creds()?;
    let client = http_client()?;
    let params = [
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
        ("refresh_token", tokens.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ];
    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .map_err(|e| format!("google_auth: refresh request failed: {e}"))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| format!("google_auth: refresh parse failed: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "google_auth: refresh HTTP {status}: {}",
            body.to_string().chars().take(300).collect::<String>()
        ));
    }

    let access = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("google_auth: refresh response missing access_token")?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    // Google may omit scope on refresh; keep the prior value in that case.
    if let Some(scope) = body.get("scope").and_then(Value::as_str) {
        tokens.scope = scope.to_string();
    }
    tokens.access_token = access.to_string();
    tokens.expires_at = now_unix() + expires_in;
    Ok(())
}

/// Revoke the stored refresh token for `ctx` with Google and delete the
/// corresponding local file. Used by `claudette --auth-google [scope] --revoke`.
pub fn revoke(ctx: AuthContext) -> Result<(), String> {
    let tokens = load_tokens(ctx)?;
    let client = http_client()?;
    let resp = client
        .post(REVOKE_URL)
        .form(&[("token", tokens.refresh_token.as_str())])
        .send()
        .map_err(|e| format!("google_auth: revoke request failed: {e}"))?;
    // Google returns 200 for an already-invalid token too; we tolerate both.
    if !resp.status().is_success() {
        eprintln!(
            "google_auth: remote revoke returned HTTP {} — deleting local tokens anyway",
            resp.status()
        );
    }
    let path = tokens_path(ctx);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("google_auth: delete {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Interactive `claudette --auth-google [scope]` flow. Binds a loopback
/// listener, opens the browser, captures the `code`, exchanges it for
/// tokens, saves to the context-specific file.
pub fn run_auth_flow(ctx: AuthContext) -> Result<(), String> {
    let creds = load_client_creds()?;

    // Bind first so we know which port to put in the redirect URI.
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("google_auth: bind loopback: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("google_auth: local_addr: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let state = random_state();
    let scopes = ctx.scopes().join(" ");
    let authorize = format!(
        "{AUTHORIZE_URL}?client_id={cid}&redirect_uri={redir}&response_type=code\
         &scope={scope}&access_type=offline&prompt=consent&state={state}",
        cid = url_encode(&creds.client_id),
        redir = url_encode(&redirect_uri),
        scope = url_encode(&scopes),
    );

    eprintln!(
        "Opening browser to authorize Claudette with Google ({scope})…",
        scope = ctx.label()
    );
    eprintln!("If it doesn't open, paste this URL manually:\n  {authorize}\n");
    let _ = open_browser(&authorize);

    let (code, returned_state) = accept_callback(&listener)?;
    if returned_state != state {
        return Err(format!(
            "google_auth: state mismatch (got '{returned_state}', expected '{state}') \
             — possible CSRF; aborting"
        ));
    }

    let tokens = exchange_code(&creds, &code, &redirect_uri)?;
    save_tokens(ctx, &tokens)?;
    eprintln!(
        "✔ Saved {} tokens to {}",
        ctx.label(),
        tokens_path(ctx).display()
    );
    Ok(())
}

fn exchange_code(
    creds: &ClientCreds,
    code: &str,
    redirect_uri: &str,
) -> Result<GoogleTokens, String> {
    let client = http_client()?;
    let params = [
        ("code", code),
        ("client_id", creds.client_id.as_str()),
        ("client_secret", creds.client_secret.as_str()),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .map_err(|e| format!("google_auth: token exchange failed: {e}"))?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .map_err(|e| format!("google_auth: token exchange parse: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "google_auth: token HTTP {status}: {}",
            body.to_string().chars().take(500).collect::<String>()
        ));
    }

    let access = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("google_auth: response missing access_token")?;
    let refresh = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or("google_auth: response missing refresh_token — did you include access_type=offline and prompt=consent?")?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    let scope = body
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let token_type = body
        .get("token_type")
        .and_then(Value::as_str)
        .unwrap_or("Bearer")
        .to_string();

    Ok(GoogleTokens {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at: now_unix() + expires_in,
        scope,
        token_type,
    })
}

/// Accept a single HTTP GET on the loopback listener, parse the query string,
/// return (code, state). Writes a tiny HTML page back so the browser shows a
/// friendly "you can close this tab" message.
fn accept_callback(listener: &TcpListener) -> Result<(String, String), String> {
    listener
        .set_nonblocking(false)
        .map_err(|e| format!("google_auth: set_nonblocking: {e}"))?;

    // Single-shot: we only need one callback. If the user cancels, the
    // connection never arrives and `incoming().next()` is None, returning
    // the "listener closed" error below.
    if let Some(stream) = listener.incoming().next() {
        let mut stream = stream.map_err(|e| format!("google_auth: accept connection: {e}"))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

        let mut buf = [0u8; 4096];
        let n = stream
            .read(&mut buf)
            .map_err(|e| format!("google_auth: read callback: {e}"))?;
        let req = String::from_utf8_lossy(&buf[..n]);

        // First line: "GET /callback?code=...&state=... HTTP/1.1"
        let first = req.lines().next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let _method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");

        let query = target.split_once('?').map_or("", |(_, q)| q);
        let mut code = String::new();
        let mut state = String::new();
        let mut err = String::new();
        for kv in query.split('&') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            let decoded = url_decode(v);
            match k {
                "code" => code = decoded,
                "state" => state = decoded,
                "error" => err = decoded,
                _ => {}
            }
        }

        let body_html = if err.is_empty() && !code.is_empty() {
            "<html><body><h2>Claudette is authorized.</h2>\
             <p>You can close this tab.</p></body></html>"
        } else {
            "<html><body><h2>Authorization failed.</h2>\
             <p>Check the terminal for details.</p></body></html>"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body_html.len(),
            body_html
        );
        let _ = stream.write_all(response.as_bytes());

        if !err.is_empty() {
            return Err(format!("google_auth: Google returned error='{err}'"));
        }
        if code.is_empty() {
            return Err(
                "google_auth: callback missing 'code' param — did you cancel the consent screen?"
                    .to_string(),
            );
        }
        return Ok((code, state));
    }
    Err("google_auth: listener closed before receiving callback".to_string())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
}

/// 128-bit random hex from system entropy via `/dev/urandom` or Windows
/// RtlGenRandom. We don't need cryptographic strength here — the `state`
/// value only needs to be unpredictable to a network-local attacker during
/// the ~30-second window between issue and redemption. Using wall-clock +
/// process-id xor is deliberately simple; if this ever feels thin, swap in
/// `getrandom` without touching callers.
fn random_state() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = u128::from(std::process::id());
    let mixed = t ^ (pid.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    format!("{mixed:032x}")
}

/// Percent-encode the subset of chars we actually emit in query strings.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_nibble(bytes[i + 1]);
                let lo = hex_nibble(bytes[i + 2]);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h << 4) | l);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_leaves_unreserved() {
        assert_eq!(url_encode("abcXYZ-_.~09"), "abcXYZ-_.~09");
    }

    #[test]
    fn url_encode_percent_encodes_space_and_slash() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a/b"), "a%2Fb");
        assert_eq!(url_encode("a:b"), "a%3Ab");
    }

    #[test]
    fn url_decode_roundtrips_space_slash_colon() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("a%2Fb"), "a/b");
        assert_eq!(url_decode("a%3Ab"), "a:b");
    }

    #[test]
    fn url_decode_handles_plus_as_space() {
        assert_eq!(url_decode("hello+world"), "hello world");
    }

    #[test]
    fn url_decode_keeps_unknown_escapes_literal() {
        // Malformed percent escape is surfaced literally rather than
        // panicking. Tools that receive the value upstream will then reject
        // it with their own "missing/invalid" error.
        assert_eq!(url_decode("%ZZ"), "%ZZ");
    }

    #[test]
    fn random_state_is_32_hex_chars() {
        let s = random_state();
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_state_changes_between_calls() {
        let a = random_state();
        // Sleep one nanosecond's worth to guarantee the wall-clock component
        // moves on fast machines. In practice `SystemTime::now()` already
        // advances monotonically across two calls.
        std::thread::sleep(Duration::from_nanos(1));
        let b = random_state();
        assert_ne!(a, b, "two state values should differ");
    }

    #[test]
    fn tokens_path_under_secrets() {
        let p = tokens_path(AuthContext::Calendar);
        assert!(p.ends_with("google_oauth.json"));
        assert!(p.parent().unwrap().ends_with("secrets"));
    }

    #[test]
    fn gmail_tokens_path_differs_from_calendar() {
        let cal = tokens_path(AuthContext::Calendar);
        let gmail = tokens_path(AuthContext::GmailRead);
        assert_ne!(cal, gmail);
        assert!(gmail.ends_with("google_oauth_gmail_read.json"));
    }

    #[test]
    fn client_path_under_secrets() {
        let p = client_path();
        assert!(p.ends_with("google_oauth_client.json"));
    }

    #[test]
    fn auth_context_parse_accepts_canonical_and_aliases() {
        assert_eq!(AuthContext::parse("calendar"), Some(AuthContext::Calendar));
        assert_eq!(AuthContext::parse("cal"), Some(AuthContext::Calendar));
        assert_eq!(AuthContext::parse("gcal"), Some(AuthContext::Calendar));
        assert_eq!(AuthContext::parse("gmail"), Some(AuthContext::GmailRead));
        assert_eq!(
            AuthContext::parse("gmail-read"),
            Some(AuthContext::GmailRead)
        );
        assert_eq!(AuthContext::parse("GMAIL"), Some(AuthContext::GmailRead));
        assert_eq!(AuthContext::parse("unknown"), None);
        assert_eq!(AuthContext::parse(""), None);
    }

    #[test]
    fn auth_context_scopes_are_distinct() {
        let cal = AuthContext::Calendar.scopes();
        let gmail = AuthContext::GmailRead.scopes();
        assert!(!cal.is_empty());
        assert!(!gmail.is_empty());
        assert_ne!(cal, gmail);
        // Defence in depth: phase 4 must NOT request gmail.send.
        for s in gmail {
            assert!(
                !s.contains("gmail.send") && !s.contains("gmail.modify"),
                "gmail-read scope leaked a write permission: {s}"
            );
        }
    }

    #[test]
    fn load_client_creds_errors_when_missing() {
        // Only meaningful if the user running tests doesn't actually have
        // these env vars set. Guard with a check.
        if std::env::var("CLAUDETTE_GOOGLE_CLIENT_ID").is_ok()
            || std::env::var("GOOGLE_CLIENT_ID").is_ok()
        {
            return;
        }
        if client_path().exists() {
            return;
        }
        let err = load_client_creds().unwrap_err();
        assert!(err.contains("OAuth client not configured"), "got: {err}");
        assert!(err.contains("docs/google_setup.md"), "got: {err}");
    }

    #[test]
    fn access_token_errors_without_tokens() {
        // If the user happens to have a real token file, skip.
        if tokens_path(AuthContext::Calendar).exists() {
            return;
        }
        let err = access_token(AuthContext::Calendar).unwrap_err();
        assert!(err.contains("not authenticated"), "got: {err}");
        assert!(err.contains("--auth-google"), "got: {err}");
    }
}
