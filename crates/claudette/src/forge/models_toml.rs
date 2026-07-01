//! TOML-based model configuration loader.
//!
//! Ported verbatim from `claudettes-forge/crates/core/src/models_toml.rs`
//! at the `rc1-final` tag. Dormant in claudette 0.4.1 — not surfaced in
//! the CLI or wired into the agent runtime. Carried for forge-mode work.
//!
//! ## Resolution chain (lowest → highest priority)
//!
//! 1. Built-in defaults (all roles → Ollama with qwen3.5 / qwen3-coder)
//! 2. TOML overlay at `~/.claudettes-forge/models.toml` (optional)
//! 3. Per-role env vars: `CLAUDETTES_FORGE_<ROLE>_MODEL` and
//!    `CLAUDETTES_FORGE_<ROLE>_PROVIDER`
//!
//! ## TOML format
//!
//! ```toml
//! # ~/.claudettes-forge/models.toml
//! [assistant]
//! model    = "qwen3.5:8b"
//! provider = "ollama"
//!
//! [coder]
//! model    = "qwen3-coder:30b"
//! provider = "ollama"
//!
//! [cto]
//! model    = "claude-opus-4-7"
//! provider = "anthropic"
//! ```
//!
//! Any omitted role keeps the built-in default. Unknown TOML keys are
//! silently ignored for forward compatibility.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::types::{ModelMap, ProviderKind, Role};

// ─── Error ───────────────────────────────────────────────────────────────────

/// Errors that can occur when loading `models.toml`.
#[derive(Debug)]
pub enum ModelsTomlError {
    /// File existed but could not be read.
    Io(String),
    /// File content is not valid TOML.
    Parse(String),
    /// A `provider` string is not one of `"ollama"` or `"anthropic"`.
    InvalidProvider {
        /// TOML section name (e.g. `"coder"`).
        role: String,
        /// The unrecognised value the user wrote.
        value: String,
    },
}

impl std::fmt::Display for ModelsTomlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "models.toml I/O error: {msg}"),
            Self::Parse(msg) => write!(f, "models.toml parse error: {msg}"),
            Self::InvalidProvider { role, value } => write!(
                f,
                "models.toml: role '{role}' has unknown provider '{value}' \
                 (use 'ollama' or 'anthropic')"
            ),
        }
    }
}

impl std::error::Error for ModelsTomlError {}

// ─── TOML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TomlFile {
    assistant: Option<TomlRole>,
    planner: Option<TomlRole>,
    router: Option<TomlRole>,
    coder: Option<TomlRole>,
    test_coder: Option<TomlRole>,
    verifier: Option<TomlRole>,
    surgical_coder: Option<TomlRole>,
    cto: Option<TomlRole>,
}

#[derive(Debug, Deserialize)]
struct TomlRole {
    model: Option<String>,
    provider: Option<String>,
}

// ─── Defaults + path ─────────────────────────────────────────────────────────

/// Built-in model map. All roles default to Ollama; defaults are tuned for an
/// 8 GB VRAM budget using qwen3.5 and qwen3-coder.
#[must_use]
pub fn default_model_map() -> ModelMap {
    let mut map = ModelMap::new();
    map.set(Role::Assistant, ProviderKind::Ollama, "qwen3.5:8b");
    map.set(Role::Planner, ProviderKind::Ollama, "qwen3.5:14b");
    map.set(Role::Router, ProviderKind::Ollama, "qwen3.5:8b");
    map.set(Role::Coder, ProviderKind::Ollama, "qwen3-coder:30b");
    map.set(Role::TestCoder, ProviderKind::Ollama, "qwen3-coder:30b");
    map.set(Role::Verifier, ProviderKind::Ollama, "qwen3.5:14b");
    map.set(Role::SurgicalCoder, ProviderKind::Ollama, "qwen3-coder:30b");
    map.set(Role::Cto, ProviderKind::Ollama, "qwen3.5:14b");
    map
}

/// Default TOML path: `~/.claudettes-forge/models.toml`.
#[must_use]
pub fn default_toml_path() -> PathBuf {
    crate::env_config::home_dir()
        .join(".claudettes-forge")
        .join("models.toml")
}

// ─── ModelMap extension ───────────────────────────────────────────────────────

impl ModelMap {
    /// Load a `ModelMap` from a TOML file.
    ///
    /// Missing file → built-in defaults + env overrides (not an error).
    /// Parse error or unrecognised provider value → `Err`.
    ///
    /// # Errors
    /// - `ModelsTomlError::Io` — file exists but could not be read.
    /// - `ModelsTomlError::Parse` — file content is not valid TOML.
    /// - `ModelsTomlError::InvalidProvider` — a `provider` field has an
    ///   unrecognised value.
    pub fn from_file(path: &Path) -> Result<Self, ModelsTomlError> {
        let mut map = default_model_map();

        if path.exists() {
            let content = std::fs::read_to_string(path)
                .map_err(|e| ModelsTomlError::Io(format!("{}: {e}", path.display())))?;
            let overlay: TomlFile = toml::from_str(&content)
                .map_err(|e| ModelsTomlError::Parse(format!("{}: {e}", path.display())))?;
            apply_toml_overlay(&mut map, overlay)?;
        }

        apply_env_overrides(&mut map);
        Ok(map)
    }

    /// Load from the default path (`~/.claudettes-forge/models.toml`).
    ///
    /// # Errors
    /// Propagates `ModelsTomlError` from [`ModelMap::from_file`].
    pub fn load() -> Result<Self, ModelsTomlError> {
        Self::from_file(&default_toml_path())
    }
}

// ─── Private helpers ─────────────────────────────────────────────────────────

fn apply_toml_overlay(map: &mut ModelMap, overlay: TomlFile) -> Result<(), ModelsTomlError> {
    apply_role(map, "assistant", Role::Assistant, overlay.assistant)?;
    apply_role(map, "planner", Role::Planner, overlay.planner)?;
    apply_role(map, "router", Role::Router, overlay.router)?;
    apply_role(map, "coder", Role::Coder, overlay.coder)?;
    apply_role(map, "test_coder", Role::TestCoder, overlay.test_coder)?;
    apply_role(map, "verifier", Role::Verifier, overlay.verifier)?;
    apply_role(
        map,
        "surgical_coder",
        Role::SurgicalCoder,
        overlay.surgical_coder,
    )?;
    apply_role(map, "cto", Role::Cto, overlay.cto)?;
    Ok(())
}

fn apply_role(
    map: &mut ModelMap,
    section: &str,
    role: Role,
    maybe_override: Option<TomlRole>,
) -> Result<(), ModelsTomlError> {
    let Some(ov) = maybe_override else {
        return Ok(());
    };

    // Clone current values before the mutable borrow via set().
    let (current_provider, current_model) = map
        .resolve(role)
        .map_or((ProviderKind::Ollama, String::new()), |(p, m)| {
            (p, m.to_string())
        });

    let provider = match ov.provider {
        Some(ref pstr) => parse_provider(section, pstr)?,
        None => current_provider,
    };
    let model = ov.model.unwrap_or(current_model);
    map.set(role, provider, model);
    Ok(())
}

fn parse_provider(section: &str, s: &str) -> Result<ProviderKind, ModelsTomlError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ollama" => Ok(ProviderKind::Ollama),
        "anthropic" | "claude" | "anthropic-claude" => Ok(ProviderKind::AnthropicClaude),
        _ => Err(ModelsTomlError::InvalidProvider {
            role: section.to_string(),
            value: s.to_string(),
        }),
    }
}

fn parse_provider_lenient(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ollama" => Some(ProviderKind::Ollama),
        "anthropic" | "claude" | "anthropic-claude" => Some(ProviderKind::AnthropicClaude),
        _ => None,
    }
}

fn apply_env_overrides(map: &mut ModelMap) {
    let role_prefixes: &[(&str, Role)] = &[
        ("ASSISTANT", Role::Assistant),
        ("PLANNER", Role::Planner),
        ("ROUTER", Role::Router),
        ("CODER", Role::Coder),
        ("TEST_CODER", Role::TestCoder),
        ("VERIFIER", Role::Verifier),
        ("SURGICAL_CODER", Role::SurgicalCoder),
        ("CTO", Role::Cto),
    ];

    for (prefix, role) in role_prefixes {
        let (mut provider, mut model) = map
            .resolve(*role)
            .map_or((ProviderKind::Ollama, String::new()), |(p, m)| {
                (p, m.to_string())
            });

        if let Ok(v) = std::env::var(format!("CLAUDETTES_FORGE_{prefix}_MODEL")) {
            if !v.trim().is_empty() {
                model = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var(format!("CLAUDETTES_FORGE_{prefix}_PROVIDER")) {
            if let Some(p) = parse_provider_lenient(&v) {
                provider = p;
            }
        }

        map.set(*role, provider, model);
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "forge-models-toml-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // Env vars are process-global — serialize tests that touch them.
    fn serial_env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn missing_file_returns_defaults() {
        let _g = serial_env_guard();
        let path = std::env::temp_dir().join("forge-models-toml-no-such-file.toml");
        assert!(!path.exists(), "pre-condition: file must not exist");
        let map = ModelMap::from_file(&path).expect("missing file should not error");
        let (kind, name) = map.resolve(Role::Assistant).expect("assistant has default");
        assert_eq!(kind, ProviderKind::Ollama);
        assert!(!name.is_empty());
    }

    #[test]
    fn round_trip_coder_override() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(
            &path,
            "[coder]\nmodel = \"deepseek-coder:33b\"\nprovider = \"ollama\"\n",
        )
        .unwrap();

        let map = ModelMap::from_file(&path).expect("should load");
        let (kind, name) = map.resolve(Role::Coder).unwrap();
        assert_eq!(kind, ProviderKind::Ollama);
        assert_eq!(name, "deepseek-coder:33b");

        // Non-overridden role keeps the built-in default.
        let (asst_kind, asst_name) = map.resolve(Role::Assistant).unwrap();
        assert_eq!(asst_kind, ProviderKind::Ollama);
        assert_eq!(asst_name, "qwen3.5:8b");
    }

    #[test]
    fn missing_role_keeps_default() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(&path, "[planner]\nmodel = \"qwen3.5:32b\"\n").unwrap();

        let map = ModelMap::from_file(&path).expect("should load");
        let (_, planner_model) = map.resolve(Role::Planner).unwrap();
        assert_eq!(planner_model, "qwen3.5:32b");

        let (_, asst_model) = map.resolve(Role::Assistant).unwrap();
        assert_eq!(asst_model, "qwen3.5:8b");
    }

    #[test]
    fn anthropic_provider_parses() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(
            &path,
            "[cto]\nmodel = \"claude-opus-4-7\"\nprovider = \"anthropic\"\n",
        )
        .unwrap();

        let map = ModelMap::from_file(&path).expect("should load");
        let (kind, name) = map.resolve(Role::Cto).unwrap();
        assert_eq!(kind, ProviderKind::AnthropicClaude);
        assert_eq!(name, "claude-opus-4-7");
    }

    #[test]
    fn invalid_toml_returns_parse_error() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(&path, "this = [invalid toml !! = 42").unwrap();
        let err = ModelMap::from_file(&path).expect_err("bad TOML should error");
        assert!(matches!(err, ModelsTomlError::Parse(_)));
    }

    #[test]
    fn invalid_provider_returns_error() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(
            &path,
            "[coder]\nmodel = \"some-model\"\nprovider = \"openai\"\n",
        )
        .unwrap();
        let err = ModelMap::from_file(&path).expect_err("unknown provider should error");
        assert!(matches!(err, ModelsTomlError::InvalidProvider { .. }));
    }

    #[test]
    fn env_override_model_precedes_toml() {
        let _g = serial_env_guard();
        let dir = tempdir();
        let path = dir.join("models.toml");
        std::fs::write(&path, "[router]\nmodel = \"toml-router:1b\"\n").unwrap();

        std::env::set_var("CLAUDETTES_FORGE_ROUTER_MODEL", "env-router:2b");
        let map = ModelMap::from_file(&path).expect("should load");
        std::env::remove_var("CLAUDETTES_FORGE_ROUTER_MODEL");

        let (_, name) = map.resolve(Role::Router).unwrap();
        assert_eq!(name, "env-router:2b");
    }

    #[test]
    fn env_override_works_without_toml_file() {
        let _g = serial_env_guard();
        let path = std::env::temp_dir().join("forge-models-toml-no-such-2.toml");
        assert!(!path.exists());

        std::env::set_var("CLAUDETTES_FORGE_VERIFIER_MODEL", "verifier-env:7b");
        std::env::set_var("CLAUDETTES_FORGE_VERIFIER_PROVIDER", "anthropic");
        let map = ModelMap::from_file(&path).expect("should load even without file");
        std::env::remove_var("CLAUDETTES_FORGE_VERIFIER_MODEL");
        std::env::remove_var("CLAUDETTES_FORGE_VERIFIER_PROVIDER");

        let (kind, name) = map.resolve(Role::Verifier).unwrap();
        assert_eq!(kind, ProviderKind::AnthropicClaude);
        assert_eq!(name, "verifier-env:7b");
    }
}
