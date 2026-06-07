//! Hardware detection — GPU VRAM probing + the VRAM→brain recommendation
//! used by `--doctor`'s "pick a brain" section and the TUI HW tab.
//!
//! Detection is a plain `nvidia-smi` shell-out (no NVML/sysinfo bindings —
//! a native dependency would violate the single-static-binary rule for a
//! nice-to-have). AMD / Apple / CPU-only boxes simply return `None` and
//! fall back to the `CLAUDETTE_VRAM_GB` env var, then 8.0 — exactly the
//! behaviour the TUI had before detection existed.

use std::process::Command;

/// Parse the output of
/// `nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits`
/// into GiB. The output is one MiB integer per GPU, one per line; we take
/// the **first** line (GPU 0 — where the brain loads). Pure so it's
/// unit-testable without a GPU.
#[must_use]
pub fn parse_nvidia_smi_mib(output: &str) -> Option<f64> {
    let first = output.lines().next()?.trim();
    let mib: f64 = first.parse().ok()?;
    if mib <= 0.0 {
        return None;
    }
    Some(mib / 1024.0)
}

/// Total VRAM of GPU 0 in GiB via `nvidia-smi`, or `None` when the binary
/// is absent / errors (AMD, Apple, CPU-only, driver hiccup).
#[must_use]
pub fn detect_vram_gb() -> Option<f64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nvidia_smi_mib(&String::from_utf8_lossy(&out.stdout))
}

/// Where a VRAM figure came from — shown to the user so they know whether
/// to trust it or set `CLAUDETTE_VRAM_GB` themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VramSource {
    /// `nvidia-smi` answered.
    Detected,
    /// `CLAUDETTE_VRAM_GB` env var.
    EnvVar,
    /// Neither — the historical 8.0 default.
    Default,
}

/// Resolve VRAM with the full fallback chain:
/// detected → `CLAUDETTE_VRAM_GB` → 8.0.
#[must_use]
pub fn resolve_vram_gb() -> (f64, VramSource) {
    if let Some(gb) = detect_vram_gb() {
        return (gb, VramSource::Detected);
    }
    if let Some(gb) = std::env::var("CLAUDETTE_VRAM_GB")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
    {
        return (gb, VramSource::EnvVar);
    }
    (8.0, VramSource::Default)
}

/// One Claudette-Certified brain recommendation. Static strings — the tier
/// table is seeded from the README's measured 50-task battery scores; keep
/// the two in sync when a new battery run changes the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrainRec {
    /// Model id in the syntax of the chosen backend (`ollama pull` id vs
    /// LM Studio `@quant`-pinned id).
    pub model: &'static str,
    /// Why this tier — score + character, straight from the README table.
    pub why: &'static str,
    /// Alternatives worth knowing about at this tier (may be empty).
    pub alternatives: &'static str,
}

/// Map (VRAM GiB, backend) → the certified brain for that tier.
///
/// Seeded from the README "Claudette Certified" table (50-task battery,
/// 2026-05-30 run) and **backend-honest**: the 92% flagship
/// `qwen3.6-35b-a3b` is distributed via LM Studio (Unsloth GGUF), NOT
/// packaged on Ollama — recommending `ollama pull qwen3.6-35b-a3b` would
/// fail, so on an Ollama backend with ≥16 GB we recommend the best
/// pullable brain and point at the backend switch. Under 16 GB,
/// `qwen3.5:4b` scores **90%** in 8 min on ~3.4 GB — it outscores both
/// `qwen3.5:9b` (88%) and `gpt-oss-20b` (86%), so it is the honest pick
/// for the whole tier, not just the floor. Advisory only — runtime model
/// selection stays with `brain_selector`.
/// The flagship tier line. A card *marketed* as 16 GB reports ~15.5–16.0
/// **GiB** through nvidia-smi (the benchmark RTX 5060 Ti 16 GB reports
/// 15.9) — a `>= 16.0` check would exclude the exact GPU the flagship was
/// certified on. 15.0 keeps every real 16 GB card in and every 12 GB card
/// (~11.7–12.0) out.
const FLAGSHIP_TIER_GIB: f64 = 15.0;

#[must_use]
pub fn recommend_brain(vram_gb: f64, openai_compat: bool) -> BrainRec {
    if vram_gb >= FLAGSHIP_TIER_GIB {
        if openai_compat {
            BrainRec {
                model: "qwen3.6-35b-a3b@q3_k_xl",
                why: "92% on the 50-task battery — best accuracy. Pin q3_k_xl: it fits \
                      16 GB; q4_k_xl spills to RAM and loses tasks to timeouts",
                alternatives: "",
            }
        } else {
            BrainRec {
                model: "qwen3.5:9b",
                why: "88% on the 50-task battery — the best brain packaged on Ollama",
                alternatives: "the 92% flagship qwen3.6-35b-a3b is LM Studio-only: set \
                               CLAUDETTE_OPENAI_COMPAT=1 + OLLAMA_HOST=http://localhost:1234 + \
                               CLAUDETTE_MODEL=qwen3.6-35b-a3b@q3_k_xl (full setup: \
                               docs/power-user.md; worth the switch on 16 GB)",
            }
        }
    } else {
        BrainRec {
            model: if openai_compat {
                "qwen3.5-4b"
            } else {
                "qwen3.5:4b"
            },
            why: "90% on the 50-task battery in 8 min on ~3.4 GB — best value; runs on an \
                  8 GB GPU or plain CPU",
            alternatives: "qwen3.5:9b (88%, 11 GB) or gpt-oss-20b (86%, 13 GB, fastest) \
                           if you have the headroom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_nvidia_smi_mib, recommend_brain, VramSource};

    #[test]
    fn parse_mib_happy_path() {
        assert_eq!(parse_nvidia_smi_mib("16384"), Some(16.0));
        assert_eq!(parse_nvidia_smi_mib("16384\n"), Some(16.0));
        assert_eq!(parse_nvidia_smi_mib("  8192  "), Some(8.0));
    }

    #[test]
    fn parse_mib_takes_first_gpu_on_multi_gpu_boxes() {
        assert_eq!(parse_nvidia_smi_mib("16384\n8192\n"), Some(16.0));
    }

    #[test]
    fn parse_mib_rejects_garbage() {
        assert_eq!(parse_nvidia_smi_mib(""), None);
        assert_eq!(parse_nvidia_smi_mib("N/A"), None);
        assert_eq!(parse_nvidia_smi_mib("[Insufficient Permissions]"), None);
        assert_eq!(parse_nvidia_smi_mib("-1"), None);
        assert_eq!(parse_nvidia_smi_mib("0"), None);
    }

    #[test]
    fn recommend_boundaries_match_the_certified_table() {
        // Flagship tier + LM Studio → the 35b, quant pinned. 15.9 is what a
        // real "16 GB" card (the benchmark RTX 5060 Ti) reports via
        // nvidia-smi — it MUST land in the flagship tier (caught live: a
        // >=16.0 check excluded the exact GPU the flagship was certified on).
        assert_eq!(recommend_brain(15.9, true).model, "qwen3.6-35b-a3b@q3_k_xl");
        assert_eq!(recommend_brain(16.0, true).model, "qwen3.6-35b-a3b@q3_k_xl");
        assert_eq!(recommend_brain(24.0, true).model, "qwen3.6-35b-a3b@q3_k_xl");
        // Flagship tier + Ollama → the flagship is NOT on Ollama; best
        // pullable brain instead, with the backend switch in the alternatives.
        let ollama16 = recommend_brain(16.0, false);
        assert_eq!(ollama16.model, "qwen3.5:9b");
        assert!(ollama16.alternatives.contains("CLAUDETTE_OPENAI_COMPAT=1"));
        // Below the line → the 4b value pick, not the 9b (4b outscores it).
        // 14.9 covers the biggest sub-16 marketing tier (a "12 GB" card
        // reports ~11.7-12.0; nothing real reports 14.9, but the boundary
        // itself must be sharp).
        assert_eq!(recommend_brain(14.9, false).model, "qwen3.5:4b");
        assert_eq!(recommend_brain(8.0, false).model, "qwen3.5:4b");
        assert_eq!(recommend_brain(7.9, true).model, "qwen3.5-4b");
        assert_eq!(recommend_brain(0.0, false).model, "qwen3.5:4b");
    }

    #[test]
    fn vram_source_is_comparable() {
        // resolve_vram_gb() itself shells out / reads env — not unit-tested.
        // This pins the enum so doctor output code can match on it.
        assert_ne!(VramSource::Detected, VramSource::Default);
    }
}
