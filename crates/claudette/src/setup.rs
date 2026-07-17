//! `claudette --setup` — the interactive first-run wizard.
//!
//! One command from a fresh install to a working first prompt: detect the
//! backend (Ollama / LM Studio), read the GPU's VRAM, offer to pull the
//! recommended Claudette-Certified brain, point at the integrations setup,
//! then finish with a full `--doctor` pass and a suggested first prompt.
//!
//! Everything here reuses probes that `--doctor` and the first-run
//! remediation already trust: [`crate::firstrun::classify_backend`] for the
//! backend/model state, [`crate::hw`] for the VRAM→brain recommendation,
//! [`crate::doctor`]'s hints and final pass. The wizard adds no new probe
//! logic — it only sequences the existing pieces interactively.
//!
//! Fail-closed posture (mirrors `firstrun::offer_fix_interactive`): stdin
//! AND stderr must be TTYs, and the wizard refuses under `--offline` (its
//! whole point is pulling/loading models, which is egress). Piped / CI /
//! offline runs get one explanatory line and exit non-zero.

use std::io::IsTerminal;
use std::process::Command;

use crate::firstrun::FirstRunCause;
use crate::theme;

/// Total number of wizard steps, for the `[n/N]` progress prefix.
const STEPS: u8 = 5;

fn step(n: u8, label: &str) {
    eprintln!();
    eprintln!("{}", theme::accent(&format!("[{n}/{STEPS}] {label}")));
}

fn note(text: &str) {
    eprintln!("  {}", theme::dim(text));
}

fn ok_line(text: &str) {
    eprintln!("  {} {}", theme::OK_GLYPH, theme::ok(text));
}

fn fix_line(cmd: &str) {
    eprintln!("      {} {}", theme::accent("↳ fix:"), theme::dim(cmd));
}

/// Entry point — returns the process exit code for the CLI (mirrors
/// `doctor::run`). `0` when the wizard ran to the end and the closing
/// doctor pass had no hard failures.
#[allow(clippy::too_many_lines)] // linear wizard script — splitting it would obscure the flow
pub fn run() -> i32 {
    theme::init();

    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        eprintln!(
            "--setup is interactive and needs a terminal on stdin/stderr — run it directly, \
             not piped or in CI. For a non-interactive report use: claudette --doctor"
        );
        return 1;
    }
    if crate::egress::is_offline() {
        eprintln!(
            "--setup is disabled under --offline: its job is pulling/loading models, which is \
             egress by definition. Check an air-gapped box with: claudette --offline --doctor"
        );
        return 1;
    }

    eprintln!(
        "{} {}",
        theme::GEAR,
        theme::brand(&format!(
            "claudette --setup (v{}) — first-run wizard",
            env!("CARGO_PKG_VERSION")
        ))
    );

    let base_url = crate::api::resolve_ollama_url();
    let compat = crate::api::resolve_openai_compat();
    let configured = crate::run::current_model();

    // ── [1/5] backend ────────────────────────────────────────────────
    let backend_label = if compat {
        "LM Studio / OpenAI-compat"
    } else {
        "Ollama"
    };
    step(1, &format!("model backend — {backend_label}"));
    let state = crate::firstrun::classify_backend(&base_url, compat, &configured);
    if state == FirstRunCause::Unreachable {
        eprintln!(
            "  {} {}",
            theme::ERR_GLYPH,
            theme::error(&format!("not reachable at {base_url}"))
        );
        fix_line(&crate::doctor::backend_start_hint(compat));
        // Everything after this step (model presence, pull offer, the
        // doctor pass) is meaningless against a dead server — stop here
        // rather than walking the user through steps that can only fail.
        note("start the server, then re-run: claudette --setup");
        return 1;
    }
    ok_line(&format!("reachable at {base_url}"));

    // ── [2/5] hardware ───────────────────────────────────────────────
    step(2, "hardware — GPU VRAM");
    let (vram_gb, source) = crate::hw::resolve_vram_gb();
    match source {
        crate::hw::VramSource::Detected => {
            ok_line(&format!("{vram_gb:.1} GiB detected via nvidia-smi"));
        }
        crate::hw::VramSource::EnvVar => {
            ok_line(&format!("{vram_gb:.1} GiB from CLAUDETTE_VRAM_GB"));
        }
        crate::hw::VramSource::Default => {
            eprintln!(
                "  {} {}",
                theme::WARN_GLYPH,
                theme::warn("no nvidia-smi (AMD/Apple/CPU?) — assuming 8 GiB")
            );
            note("set CLAUDETTE_VRAM_GB to your real figure for a better pick");
        }
    }

    // ── [3/5] recommended brain + pull offer ─────────────────────────
    let rec = crate::hw::recommend_brain(vram_gb, compat);
    step(3, &format!("recommended brain — {}", rec.model));
    note(rec.why);
    match crate::firstrun::classify_backend(&base_url, compat, rec.model) {
        FirstRunCause::Ready => {
            ok_line("already available on the backend");
        }
        FirstRunCause::Unreachable => {
            // The backend answered in step 1 and died since — rare enough
            // to just hint and move on; the doctor pass will re-check.
            fix_line(&crate::doctor::backend_start_hint(compat));
        }
        FirstRunCause::NoModelLoaded | FirstRunCause::ModelNotPulled { .. } => {
            if compat {
                // LM Studio loads happen in its GUI / `lms load` — not
                // something we can drive reliably from here (same call as
                // the first-run remediation path).
                fix_line(&crate::doctor::model_load_hint(compat, rec.model));
                note("flagship load flags (ctx / MTP): docs/hardware.md");
            } else if crate::firstrun::confirm_default_yes(&format!(
                "pull it now with `ollama pull {}`? (one-time download)",
                rec.model
            )) {
                // Inherit stdio so the user sees ollama's progress bars.
                match Command::new("ollama").args(["pull", rec.model]).status() {
                    Ok(s) if s.success() => ok_line(&format!("`{}` pulled", rec.model)),
                    Ok(s) => {
                        eprintln!(
                            "  {} {}",
                            theme::ERR_GLYPH,
                            theme::error(&format!("`ollama pull {}` exited with {s}", rec.model))
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "  {} {}",
                            theme::ERR_GLYPH,
                            theme::error(&format!("could not run `ollama` ({e})"))
                        );
                        fix_line(&crate::doctor::backend_start_hint(compat));
                    }
                }
            } else {
                note("skipped — pull it any time with the command above");
            }
        }
    }
    if !crate::doctor::model_present(std::slice::from_ref(&configured), rec.model) {
        note(&format!(
            "claudette is currently configured for `{configured}` — to switch, set \
             CLAUDETTE_MODEL={} in ~/.claudette/.env (advisory; nothing is changed for you)",
            rec.model
        ));
    }

    // ── [4/5] cloud integrations (optional) ──────────────────────────
    step(
        4,
        "cloud integrations — Telegram bot, Gmail, Calendar, voice (optional)",
    );
    note("these reach third-party services; everything so far is fully local");
    if crate::firstrun::confirm_default_yes("show how to enable them?") {
        if cfg!(feature = "integrations") {
            note("this build includes them. Next steps:");
            note("  Telegram bot:     claudette --telegram   (needs TELEGRAM_BOT_TOKEN)");
            note("  Google APIs:      claudette --auth-google calendar   (or gmail)");
            note("  full walkthrough: docs/first-success.md#assistant");
        } else {
            note("this is the lean coding-only build — grab the prebuilt full flavor:");
            note("  Linux/macOS: CLAUDETTE_FLAVOR=full curl -fsSL https://raw.githubusercontent.com/mrdushidush/claudette/main/install.sh | sh");
            note("  Windows:     $env:CLAUDETTE_FLAVOR='full'; iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex");
            note("  (or, with a Rust toolchain: cargo install claudette --features integrations)");
        }
    } else {
        note("skipping — claudette stays fully local");
    }

    // ── [5/5] doctor pass ────────────────────────────────────────────
    step(5, "final check — running the full --doctor probe");
    let code = crate::doctor::run();

    eprintln!();
    if code == 0 {
        eprintln!(
            "{} {}",
            theme::SPARKLES,
            theme::ok("setup complete — try your first prompt:")
        );
    } else {
        eprintln!(
            "{} {}",
            theme::WARN_GLYPH,
            theme::warn("setup finished with red rows above — fix those, then try:")
        );
    }
    eprintln!("      claudette \"hello — what can you do?\"");
    note("first-win recipes (coding / air-gap / assistant): docs/first-success.md");
    code
}

#[cfg(test)]
mod tests {
    #[test]
    fn setup_refuses_without_a_tty() {
        // Under `cargo test`, stdin is not a TTY — the wizard must refuse
        // with exit 1 before doing any network I/O or prompting (mirrors
        // firstrun::offer_is_refused_when_stdin_is_piped).
        assert_eq!(super::run(), 1);
    }
}
