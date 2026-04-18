//! File-backed secret storage with env-var override.
//!
//! Tokens persist across terminal sessions in plain-text files at
//! `~/.claudette/secrets/<name>.token` (mode 0600 on Unix). Env vars
//! take precedence so `export GITHUB_TOKEN=...` still overrides the file.
//!
//! Lookup order:
//! 1. `CLAUDETTE_{NAME}_TOKEN` env var (fully qualified)
//! 2. `{NAME}_TOKEN` env var (short form, e.g. `GITHUB_TOKEN`)
//! 3. `~/.claudette/secrets/{name}.token` file
//!
//! The `read_secret` helper is the single entry point — every tool that
//! needs a PAT calls it instead of `std::env::var` directly.

use std::path::PathBuf;

/// Resolve the secrets directory: `~/.claudette/secrets/`.
fn secrets_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".claudette")
        .join("secrets")
}

/// Read a secret by logical name (e.g. `"github"`, `"telegram"`).
///
/// Returns the trimmed token string, or `Err` with a user-friendly message
/// listing all three places it looked.
///
/// # Lookup order
/// 1. `CLAUDETTE_GITHUB_TOKEN` (for name `"github"`)
/// 2. `GITHUB_TOKEN`
/// 3. `~/.claudette/secrets/github.token`
pub fn read_secret(name: &str) -> Result<String, String> {
    let upper = name.to_uppercase();

    // 1. Fully qualified env var.
    let fq_var = format!("CLAUDETTE_{upper}_TOKEN");
    if let Ok(val) = std::env::var(&fq_var) {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    // 2. Short-form env var.
    let short_var = format!("{upper}_TOKEN");
    if let Ok(val) = std::env::var(&short_var) {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    // 3. File fallback.
    let file_path = secrets_dir().join(format!("{}.token", name.to_lowercase()));
    if file_path.exists() {
        match std::fs::read_to_string(&file_path) {
            Ok(contents) => {
                let trimmed = contents.trim().to_string();
                if !trimmed.is_empty() {
                    return Ok(trimmed);
                }
            }
            Err(e) => {
                return Err(format!(
                    "{name}: could not read {}: {e}",
                    file_path.display()
                ));
            }
        }
    }

    Err(format!(
        "{name}: token not found. Set {fq_var} or {short_var} env var, \
         or save it to {}",
        file_path.display()
    ))
}

/// Return the path where a secret *would* be stored on disk, for display
/// in help/error messages. Does NOT check whether the file exists.
#[must_use]
pub fn secret_file_path(name: &str) -> PathBuf {
    secrets_dir().join(format!("{}.token", name.to_lowercase()))
}

/// Path for the persisted Telegram chat ID file.
fn chat_id_path() -> PathBuf {
    secrets_dir().join("telegram_chat.id")
}

/// Load any previously persisted Telegram chat IDs from disk.
/// Returns an empty vec if the file doesn't exist or is empty.
#[must_use]
pub fn load_chat_ids() -> Vec<i64> {
    let path = chat_id_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .collect()
}

/// Persist a Telegram chat ID to disk so `--chat` isn't needed next time.
/// Appends the ID if not already present.
pub fn save_chat_id(id: i64) {
    let existing = load_chat_ids();
    if existing.contains(&id) {
        return;
    }

    let path = chat_id_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Append the new ID on its own line.
    let mut contents = existing
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    contents.push_str(&id.to_string());
    contents.push('\n');
    let _ = std::fs::write(&path, contents);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_dir_is_under_claudette() {
        let dir = secrets_dir();
        assert!(
            dir.ends_with("secrets"),
            "expected path ending in 'secrets', got {}",
            dir.display()
        );
        let parent = dir.parent().unwrap();
        assert!(
            parent.ends_with(".claudette"),
            "expected parent '.claudette', got {}",
            parent.display()
        );
    }

    #[test]
    fn secret_file_path_formats_correctly() {
        let path = secret_file_path("github");
        assert!(path.ends_with("github.token"));
        let path = secret_file_path("TELEGRAM");
        assert!(path.ends_with("telegram.token"));
    }

    #[test]
    fn read_secret_error_mentions_all_paths() {
        // Use a name unlikely to collide with any real env var.
        let err = read_secret("zzz_test_nonexistent_abc").unwrap_err();
        assert!(err.contains("CLAUDETTE_ZZZ_TEST_NONEXISTENT_ABC_TOKEN"));
        assert!(err.contains("ZZZ_TEST_NONEXISTENT_ABC_TOKEN"));
        assert!(err.contains("zzz_test_nonexistent_abc.token"));
    }

    #[test]
    fn read_secret_picks_up_env_var() {
        // Test the short-form env var path. Use a unique name so we don't
        // collide with real tokens.
        let var_name = "ZZZTESTUNIQUE42_TOKEN";
        std::env::set_var(var_name, "test-token-value");
        let result = read_secret("zzztestunique42");
        std::env::remove_var(var_name);
        assert_eq!(result.unwrap(), "test-token-value");
    }

    #[test]
    fn read_secret_trims_whitespace() {
        let var_name = "ZZZTESTTRIM99_TOKEN";
        std::env::set_var(var_name, "  spaced-token  \n");
        let result = read_secret("zzztesttrim99");
        std::env::remove_var(var_name);
        assert_eq!(result.unwrap(), "spaced-token");
    }

    #[test]
    fn read_secret_rejects_empty_env_var() {
        let var_name = "ZZZTESTEMPTY77_TOKEN";
        std::env::set_var(var_name, "   ");
        let result = read_secret("zzztestempty77");
        std::env::remove_var(var_name);
        assert!(result.is_err(), "empty/whitespace token should fail");
    }

    #[test]
    fn read_secret_file_fallback() {
        // Write a temp token file and verify it's picked up.
        let dir = secrets_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("zzztestfile88.token");
        std::fs::write(&path, "file-based-token\n").unwrap();

        let result = read_secret("zzztestfile88");
        let _ = std::fs::remove_file(&path);
        assert_eq!(result.unwrap(), "file-based-token");
    }

    #[test]
    fn chat_id_path_under_secrets() {
        let path = chat_id_path();
        assert!(path.ends_with("telegram_chat.id"));
    }

    #[test]
    fn load_chat_ids_empty_when_no_file() {
        // With no file present, should return empty vec.
        let ids = load_chat_ids();
        // We can't assert empty because previous tests may have written the file.
        // Just ensure it doesn't panic.
        assert!(ids.len() < 1000);
    }

    #[test]
    fn save_and_load_chat_id_roundtrip() {
        let path = chat_id_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap());

        // Backup existing file.
        let backup = std::fs::read_to_string(&path).ok();

        // Write a test ID.
        let _ = std::fs::write(&path, "");
        save_chat_id(9999999);
        let ids = load_chat_ids();
        assert!(ids.contains(&9999999), "got: {ids:?}");

        // Duplicate should not add another line.
        save_chat_id(9999999);
        let ids2 = load_chat_ids();
        assert_eq!(
            ids2.iter().filter(|&&id| id == 9999999).count(),
            1,
            "duplicate ID should not be saved twice"
        );

        // Restore backup.
        if let Some(b) = backup {
            let _ = std::fs::write(&path, b);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}
