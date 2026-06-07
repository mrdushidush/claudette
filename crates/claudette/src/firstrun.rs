//! First-run remediation — when the brain probe fails at startup, *offer to
//! fix it* instead of dead-ending with an error message.
//!
//! The biggest new-user drop-off is "no Ollama / no model pulled / wrong
//! model id". [`offer_fix_interactive`] re-probes the backend, classifies
//! the cause, and — **only** in an interactive terminal, and never under
//! `--offline` — offers the remediation as a `[Y/n]` prompt (`ollama pull`
//! for a missing model). Non-interactive / piped / CI runs return `false`
//! immediately, preserving the exact pre-existing behaviour: print the
//! probe error and exit non-zero.
//!
//! The cause classification is shared with `--doctor` in spirit (same
//! endpoints, same model-name matching via [`crate::doctor`]'s helpers) and
//! factored pure so the three cases are unit-testable without a server.

use std::io::IsTerminal;
use std::io::Write;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::theme;

/// Why the startup brain probe failed — the three actionable causes plus a
/// catch-all. Pure data; classification over a live server happens in
/// [`classify_backend`], over a parsed model list in
/// [`classify_models_response`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstRunCause {
    /// Backend didn't answer at all (not running / wrong port).
    Unreachable,
    /// Backend is up but its model list is empty (LM Studio with nothing
    /// loaded, fresh Ollama with nothing pulled).
    NoModelLoaded,
    /// Backend is up, has models, but the configured brain isn't among them.
    ModelNotPulled { configured: String },
    /// Backend is up and the configured brain is present — probe failure
    /// was something else (or transient); nothing for us to offer.
    Ready,
}

/// Pure classification over an already-fetched model list. Unit-tested;
/// the HTTP wrapper below stays thin and untested like doctor's probes.
#[must_use]
pub fn classify_models_response(names: &[String], configured: &str) -> FirstRunCause {
    if names.is_empty() {
        return FirstRunCause::NoModelLoaded;
    }
    if crate::doctor::model_present(names, configured) {
        return FirstRunCause::Ready;
    }
    FirstRunCause::ModelNotPulled {
        configured: configured.to_string(),
    }
}

/// Re-probe the backend and classify why startup failed. Cheap (4s timeout,
/// one GET) — runs only after `probe_ollama()` already failed, so the extra
/// request costs nothing in the happy path.
///
/// NOTE: no `egress::guard()` here because the target is the local backend
/// by construction — but the offline posture is enforced by the caller:
/// only call this after the `egress::is_offline()` gate (as
/// [`offer_fix_interactive`] does), so an offline session never probes.
#[must_use]
pub fn classify_backend(base_url: &str, openai_compat: bool, configured: &str) -> FirstRunCause {
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    else {
        return FirstRunCause::Unreachable;
    };
    let tags_url = if openai_compat {
        format!("{base_url}/v1/models")
    } else {
        format!("{base_url}/api/tags")
    };
    match client.get(&tags_url).send() {
        Ok(r) if r.status().is_success() => {
            let Ok(body) = r.json::<Value>() else {
                return FirstRunCause::Unreachable;
            };
            let names = crate::doctor::extract_model_names(&body, openai_compat);
            classify_models_response(&names, configured)
        }
        _ => FirstRunCause::Unreachable,
    }
}

/// `[Y/n]` — default **yes** (this is a helpful offer, not a danger gate;
/// the dangerous direction is doing nothing). EOF / read error / explicit
/// `n` → false.
fn confirm_default_yes(question: &str) -> bool {
    let mut err = std::io::stderr().lock();
    let _ = write!(
        err,
        "  {} {question} [Y/n] ",
        theme::warn(theme::WARN_GLYPH)
    );
    let _ = err.flush();
    let mut buf = String::new();
    match std::io::stdin().read_line(&mut buf) {
        Ok(0) => false, // EOF — non-interactive after all
        Ok(_) => {
            let a = buf.trim().to_ascii_lowercase();
            a.is_empty() || a == "y" || a == "yes"
        }
        Err(_) => false,
    }
}

/// Offer an interactive fix for a failed startup probe. Returns `true` when
/// remediation succeeded and startup can continue (verified by a fresh
/// `probe_ollama()`), `false` for "exit non-zero exactly like before".
///
/// Gates (all must hold, otherwise immediately `false` with no output):
/// - stdin AND stderr are TTYs (mirrors the forge review gate's fail-closed
///   posture toward pipes/CI — `run.rs::forge_confirm_submit`),
/// - not `--offline` (pulling a model is egress by definition).
#[must_use]
pub fn offer_fix_interactive() -> bool {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    if crate::egress::is_offline() {
        return false;
    }

    let base_url = crate::api::resolve_ollama_url();
    let compat = crate::api::resolve_openai_compat();
    let model = crate::run::current_model();

    match classify_backend(&base_url, compat, &model) {
        FirstRunCause::Unreachable => {
            // Auto-starting a long-running server is awkward (detached
            // process lifetime, GUI apps); print the copy-paste hint.
            eprintln!(
                "      {} {}",
                theme::accent("↳ fix:"),
                theme::dim(&crate::doctor::backend_start_hint(compat))
            );
            false
        }
        FirstRunCause::NoModelLoaded | FirstRunCause::ModelNotPulled { .. } => {
            if compat {
                // LM Studio loads happen in its GUI / `lms load` with the
                // model already downloaded — not something we can drive
                // reliably from here. Hint and exit.
                eprintln!(
                    "      {} {}",
                    theme::accent("↳ fix:"),
                    theme::dim(&crate::doctor::model_load_hint(compat, &model))
                );
                return false;
            }
            if !confirm_default_yes(&format!(
                "`{model}` isn't pulled — pull it now with `ollama pull {model}`?"
            )) {
                return false;
            }
            // Inherit stdio so the user sees ollama's progress bars.
            match Command::new("ollama").args(["pull", &model]).status() {
                Ok(s) if s.success() => {
                    // Verify end-to-end before letting startup continue.
                    match crate::api::probe_ollama() {
                        Ok(_) => {
                            eprintln!(
                                "  {} {}",
                                theme::OK_GLYPH,
                                theme::ok(&format!("`{model}` pulled — continuing"))
                            );
                            true
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} {}",
                                theme::ERR_GLYPH,
                                theme::error(&format!(
                                    "pull finished but the probe still fails: {e}"
                                ))
                            );
                            false
                        }
                    }
                }
                Ok(s) => {
                    eprintln!(
                        "  {} {}",
                        theme::ERR_GLYPH,
                        theme::error(&format!("`ollama pull {model}` exited with {s}"))
                    );
                    false
                }
                Err(e) => {
                    // ollama binary not found / not executable.
                    eprintln!(
                        "  {} {}",
                        theme::ERR_GLYPH,
                        theme::error(&format!("could not run `ollama` ({e})"))
                    );
                    eprintln!(
                        "      {} {}",
                        theme::accent("↳ fix:"),
                        theme::dim(&crate::doctor::backend_start_hint(compat))
                    );
                    false
                }
            }
        }
        FirstRunCause::Ready => false, // transient — let the normal error stand
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_models_response, FirstRunCause};

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn empty_list_is_no_model_loaded() {
        assert_eq!(
            classify_models_response(&[], "qwen3.5:4b"),
            FirstRunCause::NoModelLoaded
        );
    }

    #[test]
    fn missing_brain_is_model_not_pulled() {
        assert_eq!(
            classify_models_response(&names(&["llama3:8b", "phi4:14b"]), "qwen3.5:4b"),
            FirstRunCause::ModelNotPulled {
                configured: "qwen3.5:4b".to_string()
            }
        );
    }

    #[test]
    fn present_brain_is_ready() {
        assert_eq!(
            classify_models_response(&names(&["qwen3.5:4b", "phi4:14b"]), "qwen3.5:4b"),
            FirstRunCause::Ready
        );
        // Loose `:latest` matching comes from doctor::model_present.
        assert_eq!(
            classify_models_response(&names(&["qwen3.5:4b:latest"]), "qwen3.5:4b"),
            FirstRunCause::Ready
        );
    }

    #[test]
    fn offer_is_refused_when_stdin_is_piped() {
        // Under `cargo test`, stdin is not a TTY — the gate must refuse
        // before doing any network or printing anything.
        assert!(!super::offer_fix_interactive());
    }
}
