//! Secret redaction for on-disk and stderr log sinks.
//!
//! Mutating tool calls are appended verbatim to the action transcript
//! (`~/.claudette/transcript/actions.jsonl`) and the `git_*` tools echo their
//! argv + stderr to the terminal. A model that pastes a PAT into a `bash`
//! argument, a `git remote set-url https://x-access-token:<PAT>@github.com/…`,
//! or an `Authorization: Bearer …` header would otherwise persist that secret
//! in plaintext on disk and in scrollback — credential-at-rest leakage that
//! flatly contradicts the air-gap / privacy posture. [`redact`] masks the
//! common token shapes before anything reaches a sink.
//!
//! The match set is deliberately **high-precision** (named provider shapes,
//! not a generic entropy scan) so it never mangles legitimate content like
//! git hashes or base64 blobs. Each match is replaced with a
//! `<redacted:KIND>` marker so the log still reads sensibly.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Compiled `(pattern, replacement)` table, built once. Patterns are ordered
/// most-specific first; `Bearer`/`Authorization` come before the bare-token
/// shapes so the header keyword is preserved in the replacement.
fn rules() -> &'static [(Regex, &'static str)] {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| {
        // `expect` here is on compile-time-constant patterns: a malformed
        // regex is a build-time authoring bug, surfaced by the unit tests,
        // never a runtime condition.
        let r = |p: &str| Regex::new(p).expect("redaction pattern must compile");
        vec![
            // `Authorization: Bearer <token>` / `Bearer <token>` /
            // `Basic <base64>` — keep the scheme word, drop the credential.
            (
                r(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}"),
                "${1} <redacted>",
            ),
            // GitHub classic PATs and friends: ghp_, gho_, ghu_, ghs_, ghr_.
            (
                r(r"\bgh[pousr]_[A-Za-z0-9]{16,}"),
                "<redacted:github-token>",
            ),
            // GitHub *fine-grained* PATs — the current default since 2022
            // (github_pat_<22>_<59>). The bare `gh[pousr]_` rule above does NOT
            // match this prefix (the char after `gh` is `i`). (roast 2026-06-30)
            (
                r(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
                "<redacted:github-token>",
            ),
            // GitLab personal / project / group access tokens: glpat-…
            (r(r"\bglpat-[A-Za-z0-9_-]{16,}"), "<redacted:gitlab-token>"),
            // Slack tokens: xoxb-, xoxp-, xoxa-, xoxr-, xoxs-.
            (
                r(r"\bxox[baprs]-[A-Za-z0-9-]{8,}"),
                "<redacted:slack-token>",
            ),
            // AWS access-key id: AKIA + 16 upper/digit.
            (r(r"\bAKIA[0-9A-Z]{16}\b"), "<redacted:aws-key>"),
            // Google OAuth access tokens.
            (r(r"\bya29\.[A-Za-z0-9._-]{20,}"), "<redacted:google-token>"),
            // OpenAI-style and other `sk-`/`sk-proj-` API keys.
            (
                r(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}"),
                "<redacted:api-key>",
            ),
            // JSON Web Tokens: three base64url segments. The `eyJ` prefix is
            // base64 of `{"` (the JSON header), so this is high-precision and
            // won't match arbitrary dotted identifiers or git refs.
            (
                r(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
                "<redacted:jwt>",
            ),
            // PEM private-key blocks (RSA / EC / OPENSSH / PKCS#8). Spans
            // newlines via the `(?s)` dot-matches-newline flag; lazy `.*?` stops
            // at the first END line. The whole block becomes one marker.
            (
                r(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----"),
                "<redacted:private-key>",
            ),
            // `x-access-token:<pat>@` and `<user>:<pat>@github` URL creds.
            // The char classes exclude `<>` so this rule can never re-match an
            // already-substituted `<redacted:KIND>` marker (whose internal
            // colon would otherwise be read as a user:pass separator).
            (
                r(r"://[^/\s:@<>]+:[^/\s:@<>]{8,}@"),
                "://<redacted:url-credential>@",
            ),
            // Backstop for secret-bearing headers / fields that carry a value
            // with no recognised token shape: `X-Api-Key: …`, `PRIVATE-TOKEN: …`,
            // `api_key=…`. Keeps the field name + separator. The value class
            // `[^\s<]\S*` requires a non-`<` first char, so it never re-matches
            // an already-substituted `<redacted:…>` marker (keeps redaction
            // idempotent). `authorization` is intentionally omitted — the
            // Bearer/Basic and url-credential rules above already cover it, and
            // including it here would clobber the preserved scheme word. Scoped
            // to compound header/field names that don't occur in prose, so it
            // won't mangle a sentence containing "password:". (roast 2026-06-30)
            (
                r(r"(?i)\b(x-api-key|x-auth-token|private-token|api[_-]?key)(\s*[:=]\s*)[^\s<]\S*"),
                "${1}${2}<redacted:header-secret>",
            ),
        ]
    })
}

/// Mask credential-shaped substrings in `input`. Returns the original string
/// borrowed unchanged when nothing matched (the common case), so callers pay
/// no allocation on clean input.
#[must_use]
pub fn redact(input: &str) -> Cow<'_, str> {
    let mut out: Cow<'_, str> = Cow::Borrowed(input);
    for (re, repl) in rules() {
        if re.is_match(&out) {
            // `replace_all` over an owned String; only reached when a rule
            // actually fires, so clean input stays borrowed.
            out = Cow::Owned(re.replace_all(&out, *repl).into_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_input_is_borrowed_unchanged() {
        let s = "git commit -m \"fix the parser\"";
        let out = redact(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "clean input must not allocate"
        );
        assert_eq!(out, s);
    }

    #[test]
    fn masks_github_pat() {
        let out = redact("git remote add o https://ghp_ABCDEFGHIJKLMNOP0123456789@github.com/x/y");
        assert!(!out.contains("ghp_ABCDEFGHIJKLMNOP"), "got: {out}");
        assert!(out.contains("<redacted:github-token>"), "got: {out}");
    }

    #[test]
    fn masks_bearer_header_keeps_scheme() {
        let out = redact("curl -H 'Authorization: Bearer sk_live_abc123DEF456ghi789'");
        assert!(out.contains("Bearer <redacted>"), "got: {out}");
        assert!(!out.contains("abc123DEF456"), "got: {out}");
    }

    #[test]
    fn masks_slack_aws_google_openai() {
        assert!(redact("xoxb-1234567890-abcdefghij").contains("<redacted:slack-token>"));
        assert!(redact("AKIAIOSFODNN7EXAMPLE").contains("<redacted:aws-key>"));
        assert!(
            redact("token=ya29.a0AfH6SMxxxxxxxxxxxxxxxxxxxx").contains("<redacted:google-token>")
        );
        assert!(redact("OPENAI=sk-proj-abcdefghijklmnopqrstuvwx").contains("<redacted:api-key>"));
    }

    #[test]
    fn masks_github_fine_grained_pat() {
        let tok = "github_pat_11ABCDE0Y0abcdefghijkl_mnopqrstuvwxyzABCDEFGHIJ1234567890abcdEFGH";
        let input = format!("git remote set-url o https://{tok}@github.com/x/y");
        let out = redact(&input);
        assert!(!out.contains("mnopqrstuvwxyz"), "got: {out}");
        assert!(out.contains("<redacted:github-token>"), "got: {out}");
    }

    #[test]
    fn masks_gitlab_pat() {
        let out = redact("PRIVATE-TOKEN: glpat-ABCdef123456_-XYZ7890");
        assert!(!out.contains("glpat-ABCdef123456"), "got: {out}");
        assert!(out.contains("redacted"), "got: {out}");
    }

    #[test]
    fn masks_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
                   eyJzdWIiOiIxMjM0NTY3ODkwIn0.\
                   SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let input = format!("token={jwt}");
        let out = redact(&input);
        assert!(
            !out.contains("SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV"),
            "got: {out}"
        );
        assert!(out.contains("<redacted:jwt>"), "got: {out}");
    }

    #[test]
    fn masks_pem_private_key_block() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
                   b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\n\
                   AAAAAAAAABAAAABAAAA\n\
                   -----END OPENSSH PRIVATE KEY-----";
        let input = format!("here is the key:\n{pem}\nbye");
        let out = redact(&input);
        assert!(!out.contains("b3BlbnNzaC1rZXk"), "got: {out}");
        assert!(out.contains("<redacted:private-key>"), "got: {out}");
        assert!(
            out.contains("here is the key:"),
            "surrounding text kept: {out}"
        );
        assert!(out.contains("bye"), "got: {out}");
    }

    #[test]
    fn masks_bare_api_key_header() {
        let out = redact("X-Api-Key: 7c4f9a0b1d2e3f4a5b6c7d8e");
        assert!(!out.contains("7c4f9a0b1d2e"), "got: {out}");
        assert!(out.contains("<redacted:header-secret>"), "got: {out}");
        assert!(out.contains("X-Api-Key:"), "field name preserved: {out}");
    }

    #[test]
    fn masks_basic_auth_keeps_scheme() {
        let out = redact("Authorization: Basic dXNlcm5hbWU6c3VwZXJzZWNyZXQ=");
        assert!(out.contains("Basic <redacted>"), "got: {out}");
        assert!(!out.contains("c3VwZXJzZWNyZXQ"), "got: {out}");
    }

    #[test]
    fn header_backstop_is_idempotent() {
        let once = redact("api_key=supersecretvalue123456").into_owned();
        assert!(once.contains("<redacted:header-secret>"), "got: {once}");
        let twice = redact(&once);
        assert_eq!(twice, once, "re-redaction must be stable: {twice}");
    }

    #[test]
    fn does_not_mangle_prose_with_password_word() {
        // The header backstop is scoped to compound field names, so an English
        // sentence containing "password:" must survive untouched.
        let s = "Reset your password: click the link we emailed you.";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn masks_url_embedded_credentials() {
        let out = redact("https://x-access-token:ghs_supersecretvalue123456@github.com/o/r.git");
        // Either the github-token rule or the url-credential rule must hide it;
        // assert the secret value itself is gone.
        assert!(!out.contains("supersecretvalue123456"), "got: {out}");
    }

    #[test]
    fn does_not_mangle_a_git_sha_or_plain_path() {
        // A 40-char hex sha and a normal file path must survive untouched.
        let s = "diff 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b src/tools/git.rs";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = redact("Authorization: Bearer ghp_ABCDEFGHIJKLMNOP0123456789").into_owned();
        let twice = redact(&once);
        assert_eq!(
            twice, once,
            "re-redacting already-masked text must be stable"
        );
    }
}
