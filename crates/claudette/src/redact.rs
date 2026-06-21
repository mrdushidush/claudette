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
            // `Authorization: Bearer <token>` / `Bearer <token>` — keep the
            // scheme word, drop the credential.
            (
                r(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}"),
                "Bearer <redacted>",
            ),
            // GitHub PATs and friends: ghp_, gho_, ghu_, ghs_, ghr_.
            (
                r(r"\bgh[pousr]_[A-Za-z0-9]{16,}"),
                "<redacted:github-token>",
            ),
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
            // `x-access-token:<pat>@` and `<user>:<pat>@github` URL creds.
            // The char classes exclude `<>` so this rule can never re-match an
            // already-substituted `<redacted:KIND>` marker (whose internal
            // colon would otherwise be read as a user:pass separator).
            (
                r(r"://[^/\s:@<>]+:[^/\s:@<>]{8,}@"),
                "://<redacted:url-credential>@",
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
