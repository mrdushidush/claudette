//! Sprint 14 — tiered brain configuration for claudette.
//!
//! Three presets:
//! - **Fast**   — brain=qwen3.5:4b, no fallback. Pure speed, no swap-cost.
//! - **Auto**   — brain=qwen3.5:4b, fallback=qwen3.5:9b. Default. The 4b
//!   handles the 96%-baseline of prompts; the 9b acts as a safety net on
//!   the stuck signals the `brain_selector` watches for.
//! - **Smart**  — brain=qwen3.5:9b, no fallback. When the user knows the
//!   upcoming conversation is hard and wants to skip the swap dance.
//!
//! Coder is a single slot — out of scope for Sprint 14, but configured here
//! alongside brain so the whole "which models am I running" state lives in
//! one place.
//!
//! ## Resolution chain
//! Lowest→highest priority:
//! 1. Preset defaults
//! 2. TOML overlay at `~/.claudette/models.toml` (optional)
//! 3. Env vars: `CLAUDETTE_MODEL`, `CLAUDETTE_FALLBACK_BRAIN_MODEL`,
//!    `CLAUDETTE_CODER_MODEL`, plus per-role `_NUM_CTX` / `_NUM_PREDICT`
//! 4. Slash command live override (persists for the process via
//!    [`set_active`] — picked up on the next `build_runtime_streaming` call)
//!
//! Two roles: brain (conversational) + coder (Codet sidecar).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

// ─── Preset ─────────────────────────────────────────────────────────────────

/// Which tiered-brain configuration the REPL is running under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// Fast-only: qwen3.5:4b, no fallback.
    Fast,
    /// Default: qwen3.5:4b with qwen3.5:9b as the safety net.
    Auto,
    /// Heavy-only: qwen3.5:9b, no fallback.
    Smart,
}

impl std::fmt::Display for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Preset::Fast => write!(f, "fast"),
            Preset::Auto => write!(f, "auto"),
            Preset::Smart => write!(f, "smart"),
        }
    }
}

impl std::str::FromStr for Preset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "fast" => Ok(Preset::Fast),
            "auto" => Ok(Preset::Auto),
            "smart" => Ok(Preset::Smart),
            other => Err(format!(
                "unknown preset {other:?} — use fast, auto, or smart"
            )),
        }
    }
}

// ─── RoleConfig ─────────────────────────────────────────────────────────────

/// Settings for one role (brain, fallback brain, or coder). `num_ctx` and
/// `num_predict` are carried explicitly so the slash-command UI can show
/// "what's this role actually going to run with" without walking back
/// through env vars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleConfig {
    pub model: String,
    pub num_ctx: u32,
    pub num_predict: u32,
}

impl RoleConfig {
    #[must_use]
    pub fn new(model: impl Into<String>, num_ctx: u32, num_predict: u32) -> Self {
        Self {
            model: model.into(),
            num_ctx,
            num_predict,
        }
    }
}

// ─── ModelConfig ────────────────────────────────────────────────────────────

/// The resolved state of every role + whether fallback is active. Built
/// once per process at REPL startup via [`ModelConfig::resolve`] and
/// mutated in place by the `/preset`, `/brain`, `/coder` slash commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    pub preset: Preset,
    pub brain: RoleConfig,
    /// `None` for Fast/Smart, `Some(...)` for Auto. `brain_selector` reads
    /// this to decide whether to run the fallback dance on stuck signals.
    pub fallback_brain: Option<RoleConfig>,
    pub coder: RoleConfig,
}

impl ModelConfig {
    /// Preset defaults. Numbers match the env-var defaults already shipping:
    /// 16K ctx / 6K predict for brain (fits the 8 GB VRAM budget once the
    /// `q8_0` KV cache is on), 49K ctx / 12K predict for coder.
    #[must_use]
    pub fn from_preset(preset: Preset) -> Self {
        let brain_fast = RoleConfig::new("qwen3.5:4b", 16384, 6144);
        let brain_smart = RoleConfig::new("qwen3.5:9b", 16384, 6144);
        let coder = RoleConfig::new("qwen3-coder:30b", 49152, 12288);

        match preset {
            Preset::Fast => Self {
                preset,
                brain: brain_fast,
                fallback_brain: None,
                coder,
            },
            Preset::Auto => Self {
                preset,
                brain: brain_fast,
                fallback_brain: Some(brain_smart),
                coder,
            },
            Preset::Smart => Self {
                preset,
                brain: brain_smart,
                fallback_brain: None,
                coder,
            },
        }
    }

    /// Merge a TOML overlay (if the file exists). Silent on missing; logs
    /// to stderr on parse failure and returns self unchanged.
    #[must_use]
    pub fn merge_toml(mut self, path: &Path) -> Self {
        if !path.exists() {
            return self;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[model_config] read {}: {e}", path.display());
                return self;
            }
        };
        let parsed: TomlOverlay = match toml::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[model_config] parse {}: {e}", path.display());
                return self;
            }
        };

        // If the TOML names a different preset, rebuild from that preset
        // first — then layer role overrides on top.
        if let Some(preset_str) = &parsed.preset {
            if let Ok(preset) = preset_str.parse::<Preset>() {
                if preset != self.preset {
                    self = Self::from_preset(preset);
                }
            }
        }
        if let Some(ov) = parsed.brain {
            apply_role_override(&mut self.brain, ov);
        }
        if let Some(ov) = parsed.fallback_brain {
            // Auto preset may have None here; on Fast/Smart it's always
            // None. If the TOML sets one, treat it as opt-in fallback.
            let base = self
                .fallback_brain
                .clone()
                .unwrap_or_else(|| RoleConfig::new("qwen3.5:9b", 16384, 6144));
            let mut merged = base;
            apply_role_override(&mut merged, ov);
            self.fallback_brain = Some(merged);
        }
        if let Some(ov) = parsed.coder {
            apply_role_override(&mut self.coder, ov);
        }
        self
    }

    /// Merge environment overrides. Honors the existing `CLAUDETTE_MODEL`
    /// / `CLAUDETTE_NUM_CTX` / `CLAUDETTE_NUM_PREDICT` / `CLAUDETTE_CODER_*`
    /// env vars so Sprint 14 doesn't break anyone's existing setup. Adds
    /// `CLAUDETTE_FALLBACK_BRAIN_MODEL` as the new knob.
    #[must_use]
    pub fn merge_env(mut self) -> Self {
        if let Ok(v) = std::env::var("CLAUDETTE_MODEL") {
            self.brain.model = v;
        }
        if let Some(v) = env_u32("CLAUDETTE_NUM_CTX") {
            self.brain.num_ctx = v;
        }
        if let Some(v) = env_u32("CLAUDETTE_NUM_PREDICT") {
            self.brain.num_predict = v;
        }
        if let Ok(v) = std::env::var("CLAUDETTE_FALLBACK_BRAIN_MODEL") {
            if v.is_empty() {
                // Explicit empty = "turn off fallback" for this process.
                self.fallback_brain = None;
            } else {
                let base = self.fallback_brain.clone().unwrap_or_else(|| {
                    RoleConfig::new(v.clone(), self.brain.num_ctx, self.brain.num_predict)
                });
                self.fallback_brain = Some(RoleConfig {
                    model: v,
                    num_ctx: base.num_ctx,
                    num_predict: base.num_predict,
                });
            }
        }
        if let Ok(v) = std::env::var("CLAUDETTE_CODER_MODEL") {
            self.coder.model = v;
        }
        if let Some(v) = env_u32("CLAUDETTE_CODER_NUM_CTX") {
            self.coder.num_ctx = v;
        }
        if let Some(v) = env_u32("CLAUDETTE_CODER_NUM_PREDICT") {
            self.coder.num_predict = v;
        }
        self
    }

    /// Full preset → TOML → env resolution. Slash-command overrides land
    /// on the process-global state via [`set_active`] later.
    #[must_use]
    pub fn resolve(preset: Preset) -> Self {
        Self::from_preset(preset)
            .merge_toml(&default_toml_path())
            .merge_env()
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self::resolve(Preset::Auto)
    }
}

// ─── TOML schema ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TomlOverlay {
    preset: Option<String>,
    brain: Option<TomlRoleOverride>,
    fallback_brain: Option<TomlRoleOverride>,
    coder: Option<TomlRoleOverride>,
}

#[derive(Debug, Deserialize)]
struct TomlRoleOverride {
    model: Option<String>,
    num_ctx: Option<u32>,
    num_predict: Option<u32>,
}

fn apply_role_override(role: &mut RoleConfig, ov: TomlRoleOverride) {
    if let Some(m) = ov.model {
        role.model = m;
    }
    if let Some(c) = ov.num_ctx {
        role.num_ctx = c;
    }
    if let Some(p) = ov.num_predict {
        role.num_predict = p;
    }
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok().and_then(|s| s.parse::<u32>().ok())
}

/// Default path for the TOML overlay: `~/.claudette/models.toml`.
#[must_use]
pub fn default_toml_path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("models.toml")
}

// ─── Process-global active state ────────────────────────────────────────────

/// The single shared state every `build_runtime_*` call reads from. Slash
/// commands (`/preset`, `/brain`, `/coder`) mutate it; the next REPL turn
/// picks up the new values because runtimes are rebuilt on demand.
///
/// A `Mutex` behind a `OnceLock` is simpler than plumbing `Arc<Mutex<...>>`
/// through every call site that already has an organ donor's worth of
/// parameters. Reads happen once per turn — no performance concern.
fn active_cell() -> &'static Mutex<ModelConfig> {
    static CELL: OnceLock<Mutex<ModelConfig>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(ModelConfig::default()))
}

/// Read a snapshot of the currently-active config. Cheap clone (three
/// small structs).
#[must_use]
pub fn active() -> ModelConfig {
    match active_cell().lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

/// Overwrite the currently-active config. Called by the slash commands.
/// The next `build_runtime_streaming` call picks it up.
pub fn set_active(cfg: ModelConfig) {
    match active_cell().lock() {
        Ok(mut g) => *g = cfg,
        Err(p) => *p.into_inner() = cfg,
    }
}

/// Apply a mutation to the active config under the lock. Returns a clone
/// of the post-mutation state so the caller can echo what it looks like now.
pub fn update_active(f: impl FnOnce(&mut ModelConfig)) -> ModelConfig {
    match active_cell().lock() {
        Ok(mut g) => {
            f(&mut g);
            g.clone()
        }
        Err(p) => {
            let mut inner = p.into_inner();
            f(&mut inner);
            inner.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The active() state is process-global — these tests serialize their
    // access to it so parallel runs don't stomp each other.
    fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn preset_fast_has_no_fallback() {
        let cfg = ModelConfig::from_preset(Preset::Fast);
        assert_eq!(cfg.brain.model, "qwen3.5:4b");
        assert!(cfg.fallback_brain.is_none());
    }

    #[test]
    fn preset_auto_has_fallback() {
        let cfg = ModelConfig::from_preset(Preset::Auto);
        assert_eq!(cfg.brain.model, "qwen3.5:4b");
        let fb = cfg.fallback_brain.expect("Auto should have fallback");
        assert_eq!(fb.model, "qwen3.5:9b");
    }

    #[test]
    fn preset_smart_uses_9b_only() {
        let cfg = ModelConfig::from_preset(Preset::Smart);
        assert_eq!(cfg.brain.model, "qwen3.5:9b");
        assert!(cfg.fallback_brain.is_none());
    }

    #[test]
    fn preset_parse_accepts_three_values() {
        assert_eq!("fast".parse::<Preset>().unwrap(), Preset::Fast);
        assert_eq!("auto".parse::<Preset>().unwrap(), Preset::Auto);
        assert_eq!("AUTO".parse::<Preset>().unwrap(), Preset::Auto);
        assert_eq!("smart".parse::<Preset>().unwrap(), Preset::Smart);
        assert!("balanced".parse::<Preset>().is_err());
    }

    #[test]
    fn merge_toml_preset_switch() {
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(&path, "preset = \"smart\"\n").unwrap();

        let cfg = ModelConfig::from_preset(Preset::Fast).merge_toml(&path);
        assert_eq!(cfg.preset, Preset::Smart);
        assert_eq!(cfg.brain.model, "qwen3.5:9b");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn merge_toml_role_overrides() {
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(
            &path,
            r#"
[brain]
model = "qwen3.5:4b-custom"
num_ctx = 32768

[coder]
model = "qwen3-coder:14b"
num_predict = 4096
"#,
        )
        .unwrap();

        let cfg = ModelConfig::from_preset(Preset::Auto).merge_toml(&path);
        assert_eq!(cfg.brain.model, "qwen3.5:4b-custom");
        assert_eq!(cfg.brain.num_ctx, 32768);
        // num_predict untouched by toml → preset default survives.
        assert_eq!(cfg.brain.num_predict, 6144);
        assert_eq!(cfg.coder.model, "qwen3-coder:14b");
        assert_eq!(cfg.coder.num_predict, 4096);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn merge_toml_missing_file_is_noop() {
        let dir = tempdir();
        let path = dir.join("does-not-exist.toml");
        let before = ModelConfig::from_preset(Preset::Auto);
        let after = before.clone().merge_toml(&path);
        assert_eq!(before, after);
    }

    #[test]
    fn update_active_mutates_and_returns_snapshot() {
        let _g = serial_guard();
        set_active(ModelConfig::from_preset(Preset::Auto));
        let after = update_active(|cfg| {
            cfg.brain.model = "some-pinned-model".into();
            cfg.fallback_brain = None;
        });
        assert_eq!(after.brain.model, "some-pinned-model");
        assert!(after.fallback_brain.is_none());
        assert_eq!(active().brain.model, "some-pinned-model");
    }

    #[test]
    fn set_active_replaces_state() {
        let _g = serial_guard();
        set_active(ModelConfig::from_preset(Preset::Smart));
        assert_eq!(active().brain.model, "qwen3.5:9b");
        set_active(ModelConfig::from_preset(Preset::Fast));
        assert_eq!(active().brain.model, "qwen3.5:4b");
        assert!(active().fallback_brain.is_none());
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lh-model-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&d);
        d
    }
}
