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
    for raw in diff.lines() {
        if let Some(rest) = raw.strip_prefix("+++ ") {
            let p = rest.trim();
            file = p.strip_prefix("b/").unwrap_or(p).to_string();
            continue;
        }
        if raw.starts_with("+++") {
            continue;
        }
        let Some(added) = raw.strip_prefix('+') else {
            continue;
        };
        let snippet: String = added.trim().chars().take(160).collect();
        for (severity, rule, message) in classify(added) {
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

/// Apply every rule to a single added line. Returns `(severity, rule, message)`
/// for each match. Case-sensitive where the construct is (JS APIs), with a
/// lowercased copy for keyword checks.
fn classify(line: &str) -> Vec<(Severity, &'static str, &'static str)> {
    use Severity::{High, Medium};
    let l = line;
    let lower = line.to_ascii_lowercase();
    let mut v = Vec::new();

    // ── XSS sinks (JS / HTML / React) ──────────────────────────────────
    if innerhtml_assignment(l) {
        v.push((
            High,
            "xss-innerhtml",
            "assignment to innerHTML/outerHTML injects unescaped HTML (XSS); use textContent or sanitize",
        ));
    }
    if l.contains("insertAdjacentHTML(") {
        v.push((
            High,
            "xss-insertadjacenthtml",
            "insertAdjacentHTML renders raw HTML (XSS); sanitize the input first",
        ));
    }
    if l.contains("document.write(") {
        v.push((
            High,
            "xss-document-write",
            "document.write() with dynamic content is an XSS sink",
        ));
    }
    if l.contains("dangerouslySetInnerHTML") {
        v.push((
            High,
            "xss-dangerously-set-inner-html",
            "dangerouslySetInnerHTML bypasses React escaping (XSS); sanitize first",
        ));
    }
    if lower.contains("javascript:") {
        v.push((
            High,
            "xss-javascript-url",
            "javascript: URL executes script (XSS); allow-list link protocols",
        ));
    }

    // ── Arbitrary code execution ───────────────────────────────────────
    if has_call(l, "eval") {
        v.push((
            High,
            "code-eval",
            "eval() executes arbitrary code; avoid it or use a real parser",
        ));
    }
    if l.contains("new Function(") {
        v.push((
            High,
            "code-new-function",
            "new Function() compiles arbitrary code like eval()",
        ));
    }
    if l.contains("setTimeout(") && first_arg_is_string(l, "setTimeout(") {
        v.push((
            Medium,
            "code-settimeout-string",
            "string argument to setTimeout is evaluated like eval()",
        ));
    }
    if l.contains("setInterval(") && first_arg_is_string(l, "setInterval(") {
        v.push((
            Medium,
            "code-setinterval-string",
            "string argument to setInterval is evaluated like eval()",
        ));
    }

    // ── Shell / deserialization (Python & friends) ─────────────────────
    if lower.contains("shell=true") {
        v.push((
            High,
            "shell-injection",
            "subprocess with shell=True enables command injection; pass an argv list",
        ));
    }
    if l.contains("os.system(") {
        v.push((
            High,
            "shell-os-system",
            "os.system() runs a shell; use subprocess with an argv list",
        ));
    }
    if l.contains("pickle.loads(") || l.contains("pickle.load(") {
        v.push((
            High,
            "insecure-deserialization",
            "pickle deserialization executes arbitrary code on untrusted input",
        ));
    }
    if l.contains("yaml.load(") && !l.contains("Loader") {
        v.push((
            Medium,
            "insecure-yaml",
            "yaml.load() without SafeLoader can build arbitrary objects; use safe_load",
        ));
    }

    // ── Command exec (Node) ────────────────────────────────────────────
    if l.contains("child_process") && (l.contains("exec(") || l.contains("execSync(")) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(added_lines: &[&str], file: &str) -> String {
        let mut s = format!("--- a/{file}\n+++ b/{file}\n@@ -1,1 +1,{} @@\n", added_lines.len());
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
    fn ignores_innerhtml_clear() {
        let f = scan_diff(&diff(&["  list.innerHTML = '';", "  box.innerHTML = \"\";"], "a.js"));
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
        assert!(rules(&scan_diff(&diff(&["    subprocess.run(cmd, shell=True)"], "x.py")))
            .contains(&"shell-injection"));
        assert!(rules(&scan_diff(&diff(&["    os.system(f'rm {p}')"], "x.py")))
            .contains(&"shell-os-system"));
    }

    #[test]
    fn flags_pickle_and_unsafe_yaml() {
        assert!(rules(&scan_diff(&diff(&["    obj = pickle.loads(blob)"], "x.py")))
            .contains(&"insecure-deserialization"));
        assert!(rules(&scan_diff(&diff(&["    cfg = yaml.load(text)"], "x.py")))
            .contains(&"insecure-yaml"));
        // SafeLoader present → no finding.
        assert!(rules(&scan_diff(&diff(&["    cfg = yaml.load(text, Loader=yaml.SafeLoader)"], "x.py")))
            .is_empty());
    }

    #[test]
    fn flags_javascript_url_and_doc_write() {
        assert!(rules(&scan_diff(&diff(&["  a.href = 'javascript:alert(1)';"], "a.js")))
            .contains(&"xss-javascript-url"));
        assert!(rules(&scan_diff(&diff(&["  document.write(html);"], "a.js")))
            .contains(&"xss-document-write"));
    }

    #[test]
    fn flags_aws_key_shape() {
        assert!(rules(&scan_diff(&diff(&["  const k = 'AKIAIOSFODNN7EXAMPLE';"], "a.js")))
            .contains(&"aws-access-key"));
    }

    #[test]
    fn hardcoded_secret_heuristic_skips_placeholders_and_env() {
        assert!(rules(&scan_diff(&diff(&["  password = \"hunter2pass\""], "x.py")))
            .contains(&"hardcoded-secret"));
        assert!(rules(&scan_diff(&diff(&["  password = os.environ['PW']"], "x.py"))).is_empty());
        assert!(rules(&scan_diff(&diff(&["  api_key = \"your_key_here\""], "x.py"))).is_empty());
    }

    #[test]
    fn flags_sql_concat_but_not_parameterized() {
        assert!(rules(&scan_diff(&diff(&["  q = \"SELECT * FROM t WHERE id = \" + id"], "x.py")))
            .contains(&"sql-injection"));
        // Parameterized %s form must NOT be flagged (it is the safe pattern).
        assert!(rules(&scan_diff(&diff(&["  cur.execute(\"SELECT * FROM t WHERE id = %s\", (id,))"], "x.py")))
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
        let f = scan_diff(&diff(&["  el.innerHTML = x;", "  cfg = yaml.load(t)"], "a.js"));
        let fb = findings_feedback(&f);
        assert!(fb.contains("xss-innerhtml"));
        assert!(fb.contains("insecure-yaml"));
        assert!(fb.contains("textContent"));
    }
}
