//! Voice transcription via whisper.cpp (local STT).
//!
//! Pipeline: `.ogg` (Telegram voice) → `.wav` (ffmpeg) → text (whisper.cpp).
//!
//! Both `ffmpeg` and the `whisper.cpp` binary must be on PATH or configured
//! via env vars. If either is missing, transcription returns a clear error
//! so the caller can fall back or inform the user.
//!
//! Configuration:
//! - `CLAUDETTE_WHISPER_BIN` — path to the whisper.cpp `main` binary
//!   (default: `whisper` on PATH)
//! - `CLAUDETTE_WHISPER_MODEL` — path to the `.bin` GGML model file
//!   (default: `~/.claudette/models/ggml-small.bin`)

use std::path::{Path, PathBuf};
use std::process::Command;

/// Download a Telegram voice file via the Bot API `getFile` endpoint.
///
/// Returns the path to the downloaded `.ogg` file in a temp directory.
pub fn download_telegram_voice(
    http: &reqwest::blocking::Client,
    base_url: &str,
    file_id: &str,
) -> Result<PathBuf, String> {
    // Step 1: resolve file_id → file_path via getFile.
    let resp: serde_json::Value = http
        .get(format!("{base_url}/getFile"))
        .query(&[("file_id", file_id)])
        .send()
        .map_err(|e| format!("voice: getFile request failed: {e}"))?
        .json()
        .map_err(|e| format!("voice: getFile parse failed: {e}"))?;

    let file_path = resp
        .pointer("/result/file_path")
        .and_then(serde_json::Value::as_str)
        .ok_or("voice: getFile response missing file_path")?;

    // Step 2: download the actual file.
    // Telegram file URL: https://api.telegram.org/file/bot<token>/<file_path>
    // base_url is already "https://api.telegram.org/bot<token>"
    let download_url = format!("{}/{}", base_url.replace("/bot", "/file/bot"), file_path);

    let bytes = http
        .get(&download_url)
        .send()
        .map_err(|e| format!("voice: download failed: {e}"))?
        .bytes()
        .map_err(|e| format!("voice: reading file bytes failed: {e}"))?;

    // Save to temp dir.
    let tmp_dir = std::env::temp_dir().join("claudette-voice");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let ogg_path = tmp_dir.join(format!("{file_id}.ogg"));
    std::fs::write(&ogg_path, &bytes)
        .map_err(|e| format!("voice: writing ogg file failed: {e}"))?;

    Ok(ogg_path)
}

/// Path to the ffmpeg binary.
fn ffmpeg_bin() -> String {
    std::env::var("CLAUDETTE_FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// Convert an `.ogg` file to `.wav` using ffmpeg.
///
/// Returns the path to the output `.wav` file. The wav is 16 kHz mono
/// (what whisper.cpp expects).
pub fn ogg_to_wav(ogg_path: &Path) -> Result<PathBuf, String> {
    let wav_path = ogg_path.with_extension("wav");
    let bin = ffmpeg_bin();

    let output = Command::new(&bin)
        .args([
            "-y", // overwrite
            "-i",
            &ogg_path.to_string_lossy(),
            "-ar",
            "16000", // 16 kHz sample rate
            "-ac",
            "1", // mono
            "-c:a",
            "pcm_s16le",
            &wav_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| {
            format!(
                "voice: ffmpeg not found or failed to start: {e}. \
                 Install ffmpeg and ensure it's on PATH."
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "voice: ffmpeg conversion failed (exit {}): {}",
            output.status,
            stderr.chars().take(300).collect::<String>()
        ));
    }

    Ok(wav_path)
}

/// Path to the whisper.cpp binary.
fn whisper_bin() -> String {
    std::env::var("CLAUDETTE_WHISPER_BIN").unwrap_or_else(|_| "whisper-cli".to_string())
}

/// Path to the GGML model file.
fn whisper_model() -> PathBuf {
    if let Ok(path) = std::env::var("CLAUDETTE_WHISPER_MODEL") {
        return PathBuf::from(path);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".claudette")
        .join("models")
        .join("ggml-medium.bin")
}

/// Transcribe a `.wav` file to text using whisper.cpp.
///
/// `lang` controls the output language:
/// - `"en"` — auto-detect source, translate output to English (`--translate`)
/// - `"he"` — transcribe Hebrew in Hebrew (`--language he`)
/// - any other BCP-47 code — passed directly as `--language <code>`
///
/// Returns the transcribed text (trimmed).
pub fn transcribe_wav(wav_path: &Path, lang: &str) -> Result<String, String> {
    let bin = whisper_bin();
    let model = whisper_model();

    if !model.exists() {
        return Err(format!(
            "voice: whisper model not found at {}. Download ggml-medium.bin from \
             https://huggingface.co/ggerganov/whisper.cpp/tree/main and place it there, \
             or set CLAUDETTE_WHISPER_MODEL to the correct path.",
            model.display()
        ));
    }

    // Build args depending on target language.
    // English mode: detect source, translate to English.
    // Other modes: tell whisper what language to expect/output.
    let mut args: Vec<String> = vec![
        "-m".into(),
        model.to_string_lossy().into_owned(),
        "-f".into(),
        wav_path.to_string_lossy().into_owned(),
        "--no-timestamps".into(),
        "--output-txt".into(),
    ];
    if lang == "en" {
        args.push("--language".into());
        args.push("auto".into());
        args.push("--translate".into());
    } else {
        args.push("--language".into());
        args.push(lang.to_string());
    }

    let output = Command::new(&bin).args(&args).output().map_err(|e| {
        format!(
            "voice: whisper binary '{bin}' not found or failed to start: {e}. \
                 Install whisper.cpp and set CLAUDETTE_WHISPER_BIN if not on PATH."
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "voice: whisper transcription failed (exit {}): {}",
            output.status,
            stderr.chars().take(300).collect::<String>()
        ));
    }

    // whisper.cpp with --output-txt writes to <input>.txt alongside the wav.
    // Also prints to stdout. Try the file first, fall back to stdout.
    let txt_path = wav_path.with_extension("wav.txt");
    let text = if txt_path.exists() {
        std::fs::read_to_string(&txt_path)
            .map_err(|e| format!("voice: reading transcript file failed: {e}"))?
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("voice: whisper produced empty transcript".to_string());
    }

    Ok(trimmed)
}

/// Unload the current Ollama model from VRAM so whisper can use the GPU.
///
/// Ollama auto-reloads on the next chat request, so this is safe. The
/// cold-swap cost (~5-10s) is worth the 7x speedup from GPU whisper.
fn unload_ollama_model() {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("CLAUDETTE_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    // Best-effort: if Ollama isn't running or the request fails, whisper
    // will just run slower (CPU fallback).
    let _ = reqwest::blocking::Client::new()
        .post(format!("{host}/api/chat"))
        .json(&serde_json::json!({
            "model": model,
            "keep_alive": 0,
        }))
        .send();
}

/// Full pipeline: download Telegram voice → convert to wav → transcribe.
///
/// `lang`: `"en"` for English output (translate mode), `"he"` for Hebrew, etc.
///
/// Unloads the Ollama model from VRAM first so whisper.cpp can use the
/// GPU at full speed. Ollama auto-reloads on the next chat turn.
///
/// Cleans up temp files after transcription (best-effort).
pub fn transcribe_telegram_voice(
    http: &reqwest::blocking::Client,
    base_url: &str,
    file_id: &str,
    lang: &str,
) -> Result<String, String> {
    let ogg_path = download_telegram_voice(http, base_url, file_id)?;
    let wav_path = ogg_to_wav(&ogg_path)?;

    // Free VRAM for GPU whisper — Ollama will auto-reload on next chat turn.
    unload_ollama_model();

    let text = transcribe_wav(&wav_path, lang);

    // Cleanup temp files (best-effort).
    let _ = std::fs::remove_file(&ogg_path);
    let _ = std::fs::remove_file(&wav_path);
    let txt_path = wav_path.with_extension("wav.txt");
    let _ = std::fs::remove_file(&txt_path);

    text
}

/// Check whether voice transcription dependencies are available.
/// Returns `Ok(())` if both ffmpeg and whisper model are found,
/// `Err(message)` with details about what's missing.
pub fn check_voice_deps() -> Result<(), String> {
    let mut missing = Vec::new();

    // Check ffmpeg.
    let ffmpeg = ffmpeg_bin();
    match Command::new(&ffmpeg).arg("-version").output() {
        Ok(out) if out.status.success() => {}
        _ => missing.push("ffmpeg (set CLAUDETTE_FFMPEG_BIN or install on PATH)"),
    }

    // Check whisper model.
    let model = whisper_model();
    if !model.exists() {
        missing.push("whisper model (download ggml-medium.bin)");
    }

    // Check whisper binary.
    let bin = whisper_bin();
    match Command::new(&bin).arg("--help").output() {
        Ok(_) => {}
        _ => missing.push("whisper.cpp binary (set CLAUDETTE_WHISPER_BIN)"),
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("voice deps missing: {}", missing.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_model_path_is_under_claudette() {
        // Clear env to test default path.
        let prev = std::env::var("CLAUDETTE_WHISPER_MODEL").ok();
        std::env::remove_var("CLAUDETTE_WHISPER_MODEL");

        let path = whisper_model();
        assert!(
            path.to_string_lossy().contains(".claudette"),
            "default model path should be under .claudette: {}",
            path.display()
        );
        assert!(path.ends_with("ggml-medium.bin"));

        // Restore.
        if let Some(p) = prev {
            std::env::set_var("CLAUDETTE_WHISPER_MODEL", p);
        }
    }

    #[test]
    fn whisper_model_path_honors_env_var() {
        std::env::set_var("CLAUDETTE_WHISPER_MODEL", "/custom/path/model.bin");
        let path = whisper_model();
        std::env::remove_var("CLAUDETTE_WHISPER_MODEL");
        assert_eq!(path, PathBuf::from("/custom/path/model.bin"));
    }

    #[test]
    fn whisper_bin_defaults_to_whisper_cli() {
        let prev = std::env::var("CLAUDETTE_WHISPER_BIN").ok();
        std::env::remove_var("CLAUDETTE_WHISPER_BIN");

        let bin = whisper_bin();
        assert_eq!(bin, "whisper-cli");

        if let Some(p) = prev {
            std::env::set_var("CLAUDETTE_WHISPER_BIN", p);
        }
    }

    #[test]
    fn whisper_bin_honors_env_var() {
        std::env::set_var("CLAUDETTE_WHISPER_BIN", "/opt/whisper-cpp/main");
        let bin = whisper_bin();
        std::env::remove_var("CLAUDETTE_WHISPER_BIN");
        assert_eq!(bin, "/opt/whisper-cpp/main");
    }

    #[test]
    fn ogg_to_wav_fails_gracefully_without_ffmpeg() {
        // With a nonexistent file, ffmpeg should fail gracefully.
        let result = ogg_to_wav(Path::new("/tmp/nonexistent.ogg"));
        // Either ffmpeg is not installed (start error) or it fails on missing input.
        assert!(result.is_err());
    }

    #[test]
    fn transcribe_wav_fails_without_model() {
        let prev = std::env::var("CLAUDETTE_WHISPER_MODEL").ok();
        std::env::set_var("CLAUDETTE_WHISPER_MODEL", "/nonexistent/path/model.bin");

        let result = transcribe_wav(Path::new("/tmp/test.wav"), "en");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("model not found"));

        if let Some(p) = prev {
            std::env::set_var("CLAUDETTE_WHISPER_MODEL", p);
        } else {
            std::env::remove_var("CLAUDETTE_WHISPER_MODEL");
        }
    }
}
