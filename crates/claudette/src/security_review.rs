//! Opt-in security-review stage for forge-mode.
//!
//! A cheap, deterministic pattern scan over the **added** lines of the
//! Coder's diff that flags well-known injection / unsafe-API constructs
//! (XSS via `innerHTML`, `eval`, `shell=True`, hardcoded secrets, …).
//!
//! When enabled (`CLAUDETTE_FORGE_SECURITY_REVIEW=1`) the forge fix-loop
//! runs this after the Verifier; HIGH-severity findings flip the round to
//! "not passing" and are fed back to the Coder so it can remediate before
//! the PR is opened. MEDIUM / LOW findings are advisory (printed, never
//! block).
//!
//! This is intentionally a high-signal heuristic, **not** a full SAST: it
//! scans only added lines and only matches a curated set of dangerous
//! patterns, so false positives stay low. It does not replace a real
//! security audit — it catches the class of bug that "does the test pass?"
//! cannot (e.g. a markdown previewer that renders user input into
//! `innerHTML` unescaped passes its functional test but ships an XSS).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Medium => "MED",
            Self::High => "HIGH",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub rule: &'static str,
    pub message: &'static str,
    /// Repo-relative file the added line belongs to (best-effort).
    pub file: String,
    /// The offending added line, trimmed (truncated for display).
    pub line: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} — {} ({}: {})",
            self.severity.label(),
            self.rule,
            self.message,
            self.file,
            self.line
        )
    }
}

/// True when the security-review stage is enabled.
#[must_use]
pub fn enabled() -> bool {
    matches!(
        std::env::var("CLAUDETTE_FORGE_SECURITY_REVIEW").as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

/// Scan a unified diff and return findings for every added line that
/// matches a rule. Only `+` lines are considered (the change's new code);
/// context and removed lines are ignored.
#[must_use]
pub fn scan_diff(diff: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut file = String::from("?");
    let mut skip_file = false;
    for raw in diff.lines() {
        if let Some(rest) = raw.strip_prefix("+++ ") {
            let p = rest.trim();
            file = p.strip_prefix("b/").unwrap_or(p).to_string();
            // Skip test/fixture/doc/minified/lockfile paths. Now that a HIGH
            // finding actually BLOCKS submission (roast RC-C), a sink that
            // appears in a *security test*, a fixture, or prose documentation
            // must not wrongly gate the PR — these files don't ship runnable
            // production code (roast scanner M1).
            skip_file = is_excluded_path(&file);
            continue;
        }
        if raw.starts_with("+++") {
            continue;
        }
        let Some(added) = raw.strip_prefix('+') else {
            continue;
        };
        if skip_file {
            continue;
        }
        let snippet: String = added.trim().chars().take(160).collect();
        // Classify with trailing comments dropped (issue #28): a sink named only
        // in a comment/docstring must not flag, or a PR that merely *documents*
        // how to avoid a vuln is hard-rejected, training the Coder to delete
        // accurate docs. String-literal handling happens per-rule inside
        // classify (code-construct rules ignore strings; value rules don't).
        let scanned = strip_comments(added);
        for (severity, rule, message) in classify(&scanned) {
            out.push(Finding {
                severity,
                rule,
                message,
                file: file.clone(),
                line: snippet.clone(),
            });
        }
    }
    out
}

/// True for diff paths that hold test/fixture/doc/generated content rather than
/// shippable production code, so the scanner shouldn't gate on them (roast
/// scanner M1). Path-segment aware so it doesn't over-match (e.g. "latest.js"
/// is not a test file just because it contains "test").
fn is_excluded_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let filename = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    // Directory segments that mark non-production trees.
    const EXCLUDED_SEGMENTS: &[&str] = &[
        "test",
        "tests",
        "__tests__",
        "__mocks__",
        "spec",
        "specs",
        "fixture",
        "fixtures",
        "testdata",
        "doc",
        "docs",
        "examples",
        "example",
    ];
    if lower
        .split(['/', '\\'])
        .any(|seg| EXCLUDED_SEGMENTS.contains(&seg))
    {
        return true;
    }
    // Documentation / generated / lock files by extension or name. `filename`
    // is already lowercased, so the extension compare is case-insensitive.
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if matches!(ext, "md" | "markdown" | "rst" | "txt" | "lock")
        || filename.contains(".min.")
        || matches!(
            filename,
            "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "cargo.lock" | "poetry.lock"
        )
    {
        return true;
    }
    // Common test-file naming conventions: foo.test.js, foo.spec.ts,
    // test_foo.py, foo_test.go.
    filename.contains(".test.")
        || filename.contains(".spec.")
        || filename.starts_with("test_")
        || filename.contains("_test.")
}

/// Build the Coder-facing remediation feedback from a set of findings
/// (HIGH + MEDIUM). Used by the fix-loop when HIGH findings are present.
#[must_use]
pub fn findings_feedback(findings: &[Finding]) -> String {
    use std::fmt::Write as _;
    let mut s = String::from(
        "SECURITY REVIEW flagged issue(s) in your change. Fix the SOURCE so they no longer \
         appear in the diff — do not merely suppress or comment them out:\n",
    );
    for f in findings.iter().filter(|f| f.severity >= Severity::Medium) {
        let _ = write!(
            s,
            "- [{}] {} in {}: {}\n    offending line: {}\n",
            f.severity.label(),
            f.rule,
            f.file,
            f.message,
            f.line
        );
    }
    s.push_str(
        "Apply the minimal safe alternative (e.g. textContent or a sanitizer instead of \
         innerHTML; parameterized queries instead of string-built SQL; an argv list instead of \
         a shell string; never hardcode secrets).",
    );
    s
}

/// Drop a trailing line comment from a single source line, preserving string
/// literals (issue #28). A conservative single-line lexer tracks `'`/`"`/`` ` ``
/// quoting so a `//` or `#` *inside* a string (a URL, a hash color) isn't
/// mistaken for a comment. `#` is treated as a comment start only when followed
/// by whitespace/end-of-line, so JS private fields (`this.#x`) and Rust
/// attributes (`#[derive]`) aren't truncated. Comments never carry a real
/// finding, so stripping them kills the "PR documents how to avoid a vuln →
/// hard-rejected" false positive without weakening any detection.
fn strip_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n); // escaped char stays inside the string
                    }
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '#' && chars.peek().is_none_or(|n| n.is_whitespace()) {
                    break; // Python / shell / YAML line comment
                }
                if c == '/' && chars.peek() == Some(&'/') {
                    break; // C / JS / Rust / Go line comment
                }
                if c == '"' || c == '\'' || c == '`' {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// Blank out the *contents* of string literals (keeping the surrounding quotes),
/// producing a "code-only" view. Used only by the code-construct rules (issue
/// #28): a sink token like `os.system(`, `eval(`, or `.innerHTML =` inside a
/// string literal is prose/logging, never an executable sink, so those rules
/// scan this view; value-based rules (secrets, SQL, hash algorithm names,
/// `javascript:` URLs — all of which legitimately live in strings) keep scanning
/// the raw (comment-stripped) line.
fn blank_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' {
                    out.push(' ');
                    if chars.next().is_some() {
                        out.push(' ');
                    }
                } else if c == q {
                    quote = None;
                    out.push(c);
                } else {
                    out.push(' ');
                }
            }
            None => {
                if c == '"' || c == '\'' || c == '`' {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// True for a `javascript:` URL protocol (script execution), distinguished from
/// prose such as a book title "JavaScript: The Good Parts" — a real protocol URL
/// has no whitespace after the colon, prose does (issue #28).
fn javascript_url(lower: &str) -> bool {
    let mut from = 0;
    while let Some(rel) = lower[from..].find("javascript:") {
        let after = from + rel + "javascript:".len();
        if lower[after..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
        {
            return true;
        }
        from = after;
    }
    false
}

/// Apply every rule to a single added line. Returns `(severity, rule, message)`
/// for each match. Case-sensitive where the construct is (JS APIs), with a
/// lowercased copy for keyword checks.
fn classify(line: &str) -> Vec<(Severity, &'static str, &'static str)> {
    use Severity::{High, Medium};
    let l = line;
    let lower = line.to_ascii_lowercase();
    // Code-only view (string-literal contents blanked) for rules whose sink is
    // an executable construct — a mention of it inside a string is prose/logging,
    // never a real sink (issue #28). Value-based rules (secrets, SQL, hash
    // names, `javascript:` URLs) keep scanning `l`/`lower`, where their signal
    // legitimately lives inside strings.
    let code = blank_strings(l);
    let code_lower = code.to_ascii_lowercase();
    let mut v = Vec::new();

    // ── XSS sinks (JS / HTML / React) ──────────────────────────────────
    if innerhtml_assignment(&code) {
        v.push((
            High,
            "xss-innerhtml",
            "assignment to innerHTML/outerHTML injects unescaped HTML (XSS); use textContent or sanitize",
        ));
    }
    if code.contains("insertAdjacentHTML(") {
        v.push((
            High,
            "xss-insertadjacenthtml",
            "insertAdjacentHTML renders raw HTML (XSS); sanitize the input first",
        ));
    }
    if code.contains("document.write(") {
        v.push((
            High,
            "xss-document-write",
            "document.write() with dynamic content is an XSS sink",
        ));
    }
    if code.contains("dangerouslySetInnerHTML") {
        v.push((
            High,
            "xss-dangerously-set-inner-html",
            "dangerouslySetInnerHTML bypasses React escaping (XSS); sanitize first",
        ));
    }
    if javascript_url(&lower) {
        v.push((
            High,
            "xss-javascript-url",
            "javascript: URL executes script (XSS); allow-list link protocols",
        ));
    }

    // ── Arbitrary code execution ───────────────────────────────────────
    if has_call(&code, "eval") {
        v.push((
            High,
            "code-eval",
            "eval() executes arbitrary code; avoid it or use a real parser",
        ));
    }
    if code.contains("new Function(") {
        v.push((
            High,
            "code-new-function",
            "new Function() compiles arbitrary code like eval()",
        ));
    }
    if code.contains("setTimeout(") && first_arg_is_string(&code, "setTimeout(") {
        v.push((
            Medium,
            "code-settimeout-string",
            "string argument to setTimeout is evaluated like eval()",
        ));
    }
    if code.contains("setInterval(") && first_arg_is_string(&code, "setInterval(") {
        v.push((
            Medium,
            "code-setinterval-string",
            "string argument to setInterval is evaluated like eval()",
        ));
    }

    // ── Shell / deserialization (Python & friends) ─────────────────────
    if code_lower.contains("shell=true") {
        v.push((
            High,
            "shell-injection",
            "subprocess with shell=True enables command injection; pass an argv list",
        ));
    }
    if code.contains("os.system(") {
        v.push((
            High,
            "shell-os-system",
            "os.system() runs a shell; use subprocess with an argv list",
        ));
    }
    if code.contains("pickle.loads(") || code.contains("pickle.load(") {
        v.push((
            High,
            "insecure-deserialization",
            "pickle deserialization executes arbitrary code on untrusted input",
        ));
    }
    if code.contains("yaml.load(") && !code.contains("Loader") {
        v.push((
            Medium,
            "insecure-yaml",
            "yaml.load() without SafeLoader can build arbitrary objects; use safe_load",
        ));
    }

    // ── Command exec (Node) ────────────────────────────────────────────
    if code.contains("child_process") && (code.contains("exec(") || code.contains("execSync(")) {
        v.push((
            Medium,
            "command-exec",
            "child_process exec with interpolated input risks command injection; prefer execFile",
        ));
    }

    // ── Secrets ────────────────────────────────────────────────────────
    if has_aws_key(l) {
        v.push((
            High,
            "aws-access-key",
            "looks like a hardcoded AWS access key id",
        ));
    }
    if looks_like_hardcoded_secret(&lower, l) {
        v.push((
            Medium,
            "hardcoded-secret",
            "possible hardcoded credential/secret in source",
        ));
    }

    // ── SQL string building ────────────────────────────────────────────
    if looks_like_sql_concat(l, &lower) {
        v.push((
            Medium,
            "sql-injection",
            "SQL query appears string-built from variables; use parameterized queries",
        ));
    }

    // ── Arbitrary code execution (Python / PHP) ────────────────────────
    // `exec(` as a bare call is Python/PHP code execution. `something.exec(`
    // (JS regex / child_process) is excluded by `has_call` (it rejects a
    // leading `.`), and Go's `exec.Command(` has no `exec(` token.
    if has_call(&code, "exec") {
        v.push((
            High,
            "code-exec",
            "exec() runs arbitrary code from a string; avoid dynamic execution",
        ));
    }
    if code.contains("__import__(") {
        v.push((
            Medium,
            "dynamic-import",
            "__import__() with an untrusted name can load arbitrary modules",
        ));
    }

    // ── Server-side template injection (Flask/Jinja) ───────────────────
    if code.contains("render_template_string(") {
        v.push((
            High,
            "ssti",
            "render_template_string renders input as a template (SSTI→RCE); render a static template with context vars",
        ));
    }

    // ── TLS / certificate verification disabled ────────────────────────
    if tls_verification_disabled(l, &lower) {
        v.push((
            High,
            "tls-verification-disabled",
            "TLS certificate verification disabled enables MITM; never disable it in production",
        ));
    }

    // ── XXE (XML external-entity resolution enabled) ───────────────────
    if xxe_enabled(&lower) {
        v.push((
            High,
            "xxe",
            "XML parser set to resolve external entities (XXE); disable DTD/entity resolution",
        ));
    }

    // ── Weak / broken cryptography ─────────────────────────────────────
    if weak_hash(&lower) {
        v.push((
            Medium,
            "weak-hash",
            "MD5/SHA-1 are broken for security use; use SHA-256+ (or bcrypt/argon2/scrypt for passwords)",
        ));
    }
    if weak_cipher(&lower) {
        v.push((
            Medium,
            "weak-cipher",
            "DES/3DES/RC4/ECB-mode are insecure; use AES-GCM or ChaCha20-Poly1305",
        ));
    }

    // ── Request-derived sinks (SSRF / open redirect / path traversal) ──
    if ssrf(&lower) {
        v.push((
            Medium,
            "ssrf",
            "outbound request built from user input can be abused for SSRF; validate and allow-list the host",
        ));
    }
    if open_redirect(&lower) {
        v.push((
            Medium,
            "open-redirect",
            "redirect target derived from user input enables open redirect; allow-list destinations",
        ));
    }
    if path_traversal(&lower) {
        v.push((
            Medium,
            "path-traversal",
            "file path built from user input enables path traversal; resolve and confine to a base directory",
        ));
    }

    // ── Prototype pollution (JS) ───────────────────────────────────────
    if proto_pollution(l) {
        v.push((
            Medium,
            "prototype-pollution",
            "writing through __proto__/prototype from dynamic keys enables prototype pollution",
        ));
    }

    // ── NoSQL injection (Mongo & friends) ──────────────────────────────
    if nosql_injection(l, &lower) {
        v.push((
            Medium,
            "nosql-injection",
            "query operator/object built from user input (e.g. $where) enables NoSQL injection; validate types",
        ));
    }

    v
}

/// True when the line assigns to `.innerHTML` / `.outerHTML` with anything
/// other than an empty-string literal (clearing, which is safe).
fn innerhtml_assignment(l: &str) -> bool {
    for prop in [".innerHTML", ".outerHTML"] {
        if let Some(idx) = l.find(prop) {
            let after = l[idx + prop.len()..].trim_start();
            if after.starts_with("+=") {
                return true;
            }
            if after.starts_with('=') && !after.starts_with("==") {
                let rhs = after[1..].trim().trim_end_matches(';').trim();
                // Clearing with an empty literal is safe; anything else is a sink.
                if !matches!(rhs, "''" | "\"\"" | "``") {
                    return true;
                }
            }
        }
    }
    false
}

/// True when `name(` appears as a call (not a member access or a substring
/// of a longer identifier, e.g. `retrieval(`).
fn has_call(l: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    let mut start = 0;
    while let Some(rel) = l[start..].find(&needle) {
        let i = start + rel;
        let prev_ok = i == 0
            || l[..i]
                .chars()
                .last()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
        if prev_ok {
            return true;
        }
        start = i + needle.len();
    }
    false
}

/// True when the first argument after `prefix` (e.g. `setTimeout(`) is a
/// string literal.
fn first_arg_is_string(l: &str, prefix: &str) -> bool {
    if let Some(idx) = l.find(prefix) {
        let after = l[idx + prefix.len()..].trim_start();
        return after.starts_with('\'') || after.starts_with('"') || after.starts_with('`');
    }
    false
}

/// `AKIA` followed by 16 uppercase/digit chars — the AWS access-key-id shape.
fn has_aws_key(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = 0;
    while let Some(rel) = l[i..].find("AKIA") {
        let s = i + rel;
        let tail = &b[s + 4..];
        if tail.len() >= 16 && tail[..16].iter().all(u8::is_ascii_alphanumeric) {
            return true;
        }
        i = s + 4;
    }
    false
}

/// Conservative hardcoded-secret heuristic: a secret-ish key name assigned a
/// non-trivial quoted string literal that isn't an obvious placeholder/env read.
fn looks_like_hardcoded_secret(lower: &str, l: &str) -> bool {
    const KEYS: [&str; 7] = [
        "password",
        "passwd",
        "api_key",
        "apikey",
        "secret",
        "access_token",
        "private_key",
    ];
    if !KEYS.iter().any(|k| lower.contains(k)) {
        return false;
    }
    let Some(eq) = l.find(['=', ':']) else {
        return false;
    };
    let rhs = l[eq + 1..].trim().trim_start_matches('=').trim();
    let Some(quote) = rhs.chars().next().filter(|c| *c == '\'' || *c == '"') else {
        return false;
    };
    let inner = &rhs[1..];
    let Some(end) = inner.find(quote) else {
        return false;
    };
    let val = &inner[..end];
    let lowv = val.to_ascii_lowercase();
    val.len() >= 4
        && !lowv.contains("env")
        && !lowv.contains("${")
        && !lowv.contains("xxx")
        && !lowv.contains("changeme")
        && !lowv.contains("your_")
        && !lowv.contains("placeholder")
}

/// Heuristic: a SQL statement built by string concatenation / interpolation.
fn looks_like_sql_concat(l: &str, lower: &str) -> bool {
    const VERBS: [&str; 5] = ["select ", "insert ", "update ", "delete ", "drop "];
    if !VERBS.iter().any(|verb| lower.contains(verb)) {
        return false;
    }
    l.contains("\" +")
        || l.contains("' +")
        || l.contains("+ \"")
        || l.contains("+ '")
        || l.contains(".format(")
        || (l.contains("f\"") && l.contains('{'))
        || (l.contains("f'") && l.contains('{'))
}

/// True when the (lowercased) line names a request/user-input source. Used by
/// the SSRF / open-redirect / path-traversal heuristics, which only fire when a
/// dangerous sink AND a user-controlled value are visible on the same line —
/// keeping false positives low at the cost of missing cross-line taint.
fn user_input_present(lower: &str) -> bool {
    const SOURCES: &[&str] = &[
        "request.",
        "request[",
        "req.",
        "req[",
        "params[",
        "params.",
        ".query",
        "query[",
        "argv[",
        "user_input",
        "userinput",
        "$_get",
        "$_post",
        "$_request",
        "ctx.query",
    ];
    SOURCES.iter().any(|s| lower.contains(s))
}

/// TLS/cert verification turned off across common stacks (requests, httpx,
/// Node, Go, Python ssl, libcurl). HIGH: this is an unambiguous MITM exposure.
fn tls_verification_disabled(l: &str, lower: &str) -> bool {
    lower.contains("verify=false")
        || lower.contains("verify = false")
        || lower.contains("rejectunauthorized:false")
        || lower.contains("rejectunauthorized: false")
        || (lower.contains("node_tls_reject_unauthorized") && lower.contains('0'))
        || lower.contains("insecureskipverify:true")
        || lower.contains("insecureskipverify: true")
        || lower.contains("insecureskipverify=true")
        || lower.contains("insecureskipverify = true")
        || l.contains("_create_unverified_context")
        || lower.contains("check_hostname=false")
        || lower.contains("check_hostname = false")
        || (lower.contains("curlopt_ssl_verifypeer")
            && (lower.contains(", 0") || lower.contains(",0") || lower.contains("false")))
}

/// XML parser explicitly configured to resolve external entities (lxml /
/// libxml2 / SAX feature flags). HIGH: enables XXE.
fn xxe_enabled(lower: &str) -> bool {
    lower.contains("resolve_entities=true")
        || lower.contains("resolve_entities = true")
        || lower.contains("noent=true")
        || lower.contains("noent = true")
        || lower.contains("load_dtd=true")
        || lower.contains("load_dtd = true")
        || lower.contains("feature_external_ges, true")
        || lower.contains("feature_external_ges,true")
}

/// MD5 / SHA-1 used via a hashing API (not just the bare word, to avoid
/// flagging comments / identifiers that merely contain "md5").
fn weak_hash(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "hashlib.md5(",
        "hashlib.sha1(",
        "createhash('md5')",
        "createhash(\"md5\")",
        "createhash('sha1')",
        "createhash(\"sha1\")",
        "messagedigest.getinstance(\"md5\")",
        "messagedigest.getinstance(\"sha1\")",
        "md5.new(",
        "sha1.new(",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// Insecure ciphers / modes named as quoted algorithm strings.
fn weak_cipher(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "\"des\"",
        "'des'",
        "\"3des\"",
        "'3des'",
        "des-cbc",
        "desede",
        "triple_des",
        "\"rc4\"",
        "'rc4'",
        "arcfour",
        "/ecb",
        "aes-128-ecb",
        "aes-256-ecb",
        "aes_ecb",
        "modeofoperation.ecb",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// An outbound HTTP fetch whose URL line also references user input → SSRF.
fn ssrf(lower: &str) -> bool {
    const FETCHERS: &[&str] = &[
        "requests.get(",
        "requests.post(",
        "requests.request(",
        "urlopen(",
        "axios.get(",
        "axios.post(",
        "http.get(",
        "httpx.get(",
        "fetch(",
    ];
    FETCHERS.iter().any(|f| lower.contains(f)) && user_input_present(lower)
}

/// A redirect whose destination comes from user input → open redirect. Safe
/// server-side route builders (`url_for` / `reverse`) are excluded.
fn open_redirect(lower: &str) -> bool {
    if !lower.contains("redirect(") {
        return false;
    }
    if lower.contains("url_for(") || lower.contains("reverse(") {
        return false;
    }
    user_input_present(lower)
}

/// A filesystem sink whose path references user input → path traversal.
fn path_traversal(lower: &str) -> bool {
    const SINKS: &[&str] = &[
        "open(",
        "send_file(",
        "sendfile(",
        "createreadstream(",
        "readfilesync(",
        "readfile(",
        "os.path.join(",
        "path.join(",
    ];
    SINKS.iter().any(|s| lower.contains(s)) && user_input_present(lower)
}

/// Writes through `__proto__` / `prototype` from dynamic keys (JS prototype
/// pollution). Referencing `__proto__` in source is rare outside this bug class.
fn proto_pollution(l: &str) -> bool {
    l.contains("__proto__")
        || l.contains(".prototype[")
        || l.contains("[\"prototype\"]")
        || l.contains("['prototype']")
}

/// Mongo-style query built from user input: a `$where` operator, or a
/// `find`/`aggregate` call interpolating a user value into the query object.
fn nosql_injection(l: &str, lower: &str) -> bool {
    lower.contains("$where")
        || ((lower.contains(".find(")
            || lower.contains(".findone(")
            || lower.contains(".aggregate("))
            && l.contains("${")
            && user_input_present(lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(added_lines: &[&str], file: &str) -> String {
        let mut s = format!(
            "--- a/{file}\n+++ b/{file}\n@@ -1,1 +1,{} @@\n",
            added_lines.len()
        );
        for line in added_lines {
            s.push('+');
            s.push_str(line);
            s.push('\n');
        }
        s
    }

    fn rules(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule).collect()
    }

    #[test]
    fn flags_innerhtml_assignment_xss() {
        let f = scan_diff(&diff(&["  el.innerHTML = userInput;"], "src/app.js"));
        assert!(rules(&f).contains(&"xss-innerhtml"));
        assert_eq!(f[0].severity, Severity::High);
        assert_eq!(f[0].file, "src/app.js");
    }

    #[test]
    fn does_not_flag_sinks_mentioned_in_comments_or_strings() {
        // issue #28: the three reproduced false positives. A sink named only in
        // a comment, docstring, log, or string literal must NOT gate the PR.
        let cases = [
            "// never do el.innerHTML = userInput; use textContent",
            "const msg = \"do not call os.system() in prod\";",
            "const title = \"JavaScript: The Good Parts\";",
            "# os.system() is dangerous — we use subprocess.run([...]) instead",
            "log.warn(\"refusing eval() of user input\")",
        ];
        for line in cases {
            let f = scan_diff(&diff(&[line], "src/app.js"));
            assert!(
                f.is_empty(),
                "comment/string mention must not flag: {line:?} → {:?}",
                rules(&f)
            );
        }
    }

    #[test]
    fn still_flags_real_sinks_after_strip() {
        // The strip must not hide genuine sinks (code outside comments/strings).
        let f = scan_diff(&diff(
            &["  document.getElementById('x').innerHTML = data;"],
            "src/app.js",
        ));
        assert!(
            rules(&f).contains(&"xss-innerhtml"),
            "real innerHTML sink must still flag: {:?}",
            rules(&f)
        );
        // os.system as a real call still flags even with a trailing comment.
        let f2 = scan_diff(&diff(&["  os.system(cmd)  # run it"], "src/run.py"));
        assert!(
            rules(&f2).contains(&"shell-os-system"),
            "real os.system call must still flag: {:?}",
            rules(&f2)
        );
    }

    #[test]
    fn excludes_test_fixture_and_doc_paths() {
        // roast scanner M1: now that HIGH blocks submission, a sink in a test
        // file / fixture / doc must NOT gate the PR.
        let sink = &["  el.innerHTML = userInput;"];
        for path in [
            "src/app.test.js",
            "tests/xss.js",
            "__tests__/render.js",
            "spec/render_spec.js",
            "fixtures/payloads.js",
            "test_render.py",
            "render_test.go",
            "docs/security.md",
            "README.md",
            "dist/bundle.min.js",
            "package-lock.json",
        ] {
            assert!(
                scan_diff(&diff(sink, path)).is_empty(),
                "{path} should be excluded from scanning"
            );
        }
    }

    #[test]
    fn does_not_over_exclude_production_paths() {
        // Substring "test"/"doc" inside a normal filename must NOT exclude it.
        for path in ["src/latest.js", "lib/document_store.js", "app/contest.py"] {
            assert!(
                !scan_diff(&diff(&["  el.innerHTML = userInput;"], path)).is_empty(),
                "{path} must still be scanned"
            );
        }
    }

    #[test]
    fn ignores_innerhtml_clear() {
        let f = scan_diff(&diff(
            &["  list.innerHTML = '';", "  box.innerHTML = \"\";"],
            "a.js",
        ));
        assert!(f.is_empty(), "clearing innerHTML must not flag: {f:?}");
    }

    #[test]
    fn ignores_innerhtml_equality_comparison() {
        let f = scan_diff(&diff(&["  if (el.innerHTML == cached) return;"], "a.js"));
        assert!(rules(&f).is_empty(), "comparison must not flag: {f:?}");
    }

    #[test]
    fn flags_eval_but_not_retrieval() {
        assert!(rules(&scan_diff(&diff(&["  eval(payload);"], "a.js"))).contains(&"code-eval"));
        assert!(rules(&scan_diff(&diff(&["  doRetrieval(x);"], "a.js"))).is_empty());
    }

    #[test]
    fn flags_shell_true_and_os_system() {
        assert!(rules(&scan_diff(&diff(
            &["    subprocess.run(cmd, shell=True)"],
            "x.py"
        )))
        .contains(&"shell-injection"));
        assert!(
            rules(&scan_diff(&diff(&["    os.system(f'rm {p}')"], "x.py")))
                .contains(&"shell-os-system")
        );
    }

    #[test]
    fn flags_pickle_and_unsafe_yaml() {
        assert!(
            rules(&scan_diff(&diff(&["    obj = pickle.loads(blob)"], "x.py")))
                .contains(&"insecure-deserialization")
        );
        assert!(
            rules(&scan_diff(&diff(&["    cfg = yaml.load(text)"], "x.py")))
                .contains(&"insecure-yaml")
        );
        // SafeLoader present → no finding.
        assert!(rules(&scan_diff(&diff(
            &["    cfg = yaml.load(text, Loader=yaml.SafeLoader)"],
            "x.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_javascript_url_and_doc_write() {
        assert!(rules(&scan_diff(&diff(
            &["  a.href = 'javascript:alert(1)';"],
            "a.js"
        )))
        .contains(&"xss-javascript-url"));
        assert!(
            rules(&scan_diff(&diff(&["  document.write(html);"], "a.js")))
                .contains(&"xss-document-write")
        );
    }

    #[test]
    fn flags_aws_key_shape() {
        assert!(rules(&scan_diff(&diff(
            &["  const k = 'AKIAIOSFODNN7EXAMPLE';"],
            "a.js"
        )))
        .contains(&"aws-access-key"));
    }

    #[test]
    fn hardcoded_secret_heuristic_skips_placeholders_and_env() {
        assert!(
            rules(&scan_diff(&diff(&["  password = \"hunter2pass\""], "x.py")))
                .contains(&"hardcoded-secret")
        );
        assert!(rules(&scan_diff(&diff(
            &["  password = os.environ['PW']"],
            "x.py"
        )))
        .is_empty());
        assert!(rules(&scan_diff(&diff(
            &["  api_key = \"your_key_here\""],
            "x.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_sql_concat_but_not_parameterized() {
        assert!(rules(&scan_diff(&diff(
            &["  q = \"SELECT * FROM t WHERE id = \" + id"],
            "x.py"
        )))
        .contains(&"sql-injection"));
        // Parameterized %s form must NOT be flagged (it is the safe pattern).
        assert!(rules(&scan_diff(&diff(
            &["  cur.execute(\"SELECT * FROM t WHERE id = %s\", (id,))"],
            "x.py"
        )))
        .is_empty());
    }

    #[test]
    fn only_added_lines_are_scanned() {
        // A removed line containing eval must be ignored.
        let d = "--- a/x.js\n+++ b/x.js\n@@ -1,2 +1,1 @@\n-eval(old);\n+safe();\n";
        assert!(scan_diff(d).is_empty());
    }

    #[test]
    fn feedback_lists_high_and_medium() {
        let f = scan_diff(&diff(
            &["  el.innerHTML = x;", "  cfg = yaml.load(t)"],
            "a.js",
        ));
        let fb = findings_feedback(&f);
        assert!(fb.contains("xss-innerhtml"));
        assert!(fb.contains("insecure-yaml"));
        assert!(fb.contains("textContent"));
    }

    #[test]
    fn flags_python_exec_and_dynamic_import() {
        assert!(rules(&scan_diff(&diff(&["    exec(user_code)"], "x.py"))).contains(&"code-exec"));
        assert!(
            rules(&scan_diff(&diff(&["    m = __import__(name)"], "x.py")))
                .contains(&"dynamic-import")
        );
        // Member-access exec (JS regex / child_process) must NOT flag as code-exec.
        assert!(
            !rules(&scan_diff(&diff(&["  const m = re.exec(s);"], "a.js"))).contains(&"code-exec")
        );
        assert!(!rules(&scan_diff(&diff(&["  child.exec(cmd);"], "a.js"))).contains(&"code-exec"));
    }

    #[test]
    fn flags_ssti_render_template_string() {
        assert!(rules(&scan_diff(&diff(
            &["    return render_template_string(page)"],
            "app.py"
        )))
        .contains(&"ssti"));
        // A static template render is the safe pattern.
        assert!(rules(&scan_diff(&diff(
            &["    return render_template('page.html', name=name)"],
            "app.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_tls_verification_disabled() {
        for line in [
            "  r = requests.get(url, verify=False)",
            "  const a = axios.create({ httpsAgent: new https.Agent({ rejectUnauthorized: false }) });",
            "  tr := &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}}",
            "  ctx = ssl._create_unverified_context()",
            "  curl_setopt(c, CURLOPT_SSL_VERIFYPEER, 0)",
        ] {
            assert!(
                rules(&scan_diff(&diff(&[line], "x.py"))).contains(&"tls-verification-disabled"),
                "should flag: {line}"
            );
        }
        // Verification left on must NOT flag.
        assert!(rules(&scan_diff(&diff(
            &["  r = requests.get(url, verify=True)"],
            "x.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_xxe_entity_resolution() {
        assert!(rules(&scan_diff(&diff(
            &["  parser = etree.XMLParser(resolve_entities=True)"],
            "x.py"
        )))
        .contains(&"xxe"));
        // Default (entities not resolved) must NOT flag.
        assert!(rules(&scan_diff(&diff(
            &["  parser = etree.XMLParser(resolve_entities=False)"],
            "x.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_weak_hash_and_cipher() {
        assert!(rules(&scan_diff(&diff(
            &["  h = hashlib.md5(data).hexdigest()"],
            "x.py"
        )))
        .contains(&"weak-hash"));
        assert!(rules(&scan_diff(&diff(
            &["  const h = crypto.createHash('sha1');"],
            "a.js"
        )))
        .contains(&"weak-hash"));
        assert!(rules(&scan_diff(&diff(
            &["  cipher = Cipher.getInstance(\"DES\");"],
            "A.java"
        )))
        .contains(&"weak-cipher"));
        assert!(rules(&scan_diff(&diff(
            &["  c = Cipher.getInstance(\"AES/ECB/PKCS5Padding\");"],
            "A.java"
        )))
        .contains(&"weak-cipher"));
        // SHA-256 is fine.
        assert!(rules(&scan_diff(&diff(&["  h = hashlib.sha256(data)"], "x.py"))).is_empty());
    }

    #[test]
    fn flags_ssrf_open_redirect_path_traversal_with_user_input() {
        assert!(rules(&scan_diff(&diff(
            &["  r = requests.get(request.args['url'])"],
            "app.py"
        )))
        .contains(&"ssrf"));
        assert!(rules(&scan_diff(&diff(
            &["  res.redirect(req.query.next);"],
            "a.js"
        )))
        .contains(&"open-redirect"));
        assert!(rules(&scan_diff(&diff(
            &["  return send_file(request.args.get('name'))"],
            "app.py"
        )))
        .contains(&"path-traversal"));
    }

    #[test]
    fn request_sinks_without_user_input_do_not_flag() {
        // A static URL / route / path must NOT trip the user-input heuristics.
        assert!(rules(&scan_diff(&diff(
            &["  r = requests.get('https://api.example.com/health')"],
            "app.py"
        )))
        .is_empty());
        assert!(rules(&scan_diff(&diff(
            &["  return redirect(url_for('home'))"],
            "app.py"
        )))
        .is_empty());
        assert!(rules(&scan_diff(&diff(
            &["  p = path.join(__dirname, 'static')"],
            "a.js"
        )))
        .is_empty());
        assert!(rules(&scan_diff(&diff(
            &["  with open('config.json') as f:"],
            "app.py"
        )))
        .is_empty());
    }

    #[test]
    fn flags_prototype_pollution_and_nosql() {
        assert!(rules(&scan_diff(&diff(
            &["  target[key].__proto__[prop] = value;"],
            "a.js"
        )))
        .contains(&"prototype-pollution"));
        assert!(rules(&scan_diff(&diff(
            &["  db.users.find({ $where: 'this.name == \\'' + name + '\\'' })"],
            "a.js"
        )))
        .contains(&"nosql-injection"));
    }
}
