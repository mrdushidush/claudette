//! Text-to-speech via Microsoft Edge TTS (free neural voices, no account needed).
//!
//! Pipeline: text → edge-tts (MP3) → ffmpeg (OGG/OPUS) → Telegram sendVoice.
//!
//! Voices used:
//! - English: `en-US-AriaNeural` (female, natural)
//! - Hebrew:  `he-IL-HilaNeural` (female)
//!
//! Configuration:
//! - `CLAUDETTE_TTS_VOICE_EN` — override English voice
//! - `CLAUDETTE_TTS_VOICE_HE` — override Hebrew voice
//! - `CLAUDETTE_TTS_MAX_CHARS` — max response length to synthesize (default 500)

use std::path::{Path, PathBuf};
use std::process::Command;

/// Maximum response length (chars) that gets synthesized. Longer responses
/// are text-only — nobody wants to listen to a 2000-char research dump.
fn tts_max_chars() -> usize {
    std::env::var("CLAUDETTE_TTS_MAX_CHARS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
}

/// Pick the Edge TTS voice for a given language code.
pub fn voice_for_lang(lang: &str) -> &'static str {
    match lang {
        "he" => "he-IL-HilaNeural",
        _ => "en-US-AvaNeural",
    }
}

/// Synthesize `text` to an MP3 file using edge-tts.
///
/// Returns the path to the generated MP3. Caller is responsible for cleanup.
fn synthesize_to_mp3(text: &str, lang: &str, out_path: &Path) -> Result<(), String> {
    let voice = match lang {
        "he" => std::env::var("CLAUDETTE_TTS_VOICE_HE")
            .unwrap_or_else(|_| "he-IL-HilaNeural".to_string()),
        _ => std::env::var("CLAUDETTE_TTS_VOICE_EN")
            .unwrap_or_else(|_| "en-US-AvaNeural".to_string()),
    };

    let output = Command::new("python")
        .args([
            "-m",
            "edge_tts",
            "--voice",
            &voice,
            "--text",
            text,
            "--write-media",
            &out_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("tts: edge-tts failed to start: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tts: edge-tts failed (exit {}): {}",
            output.status,
            stderr.chars().take(200).collect::<String>()
        ));
    }

    if !out_path.exists() {
        return Err("tts: edge-tts produced no output file".to_string());
    }

    Ok(())
}

/// Convert an MP3 to OGG/OPUS for Telegram's `sendVoice` API.
fn mp3_to_ogg(mp3_path: &Path, ogg_path: &Path) -> Result<(), String> {
    let ffmpeg = std::env::var("CLAUDETTE_FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string());

    let output = Command::new(&ffmpeg)
        .args([
            "-y",
            "-i",
            &mp3_path.to_string_lossy(),
            "-c:a",
            "libopus",
            "-b:a",
            "64k",
            &ogg_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("tts: ffmpeg not found: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tts: ffmpeg conversion failed: {}",
            stderr.chars().take(200).collect::<String>()
        ));
    }

    Ok(())
}

/// Strip markdown formatting so the TTS voice doesn't read out
/// `asterisk asterisk bold asterisk asterisk`. Removes headers, bold,
/// italic, inline code, bullet markers, and links.
fn strip_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let line = line.trim();
        // Skip pure markdown lines (headers, horizontal rules).
        if line.starts_with('#') || line == "---" || line == "***" {
            // Convert headers to a brief pause by adding a period.
            out.push_str(line.trim_start_matches('#').trim());
            out.push_str(". ");
            continue;
        }
        // Strip inline markdown: **bold**, *italic*, `code`, [link](url).
        let mut s = line.to_string();
        for ch in ['*', '`'] {
            s = s.replace(ch, "");
        }
        // Remove markdown links [text](url) → text.
        while let Some(start) = s.find('[') {
            if let Some(mid) = s[start..].find("](") {
                if let Some(end) = s[start + mid..].find(')') {
                    let link_text = s[start + 1..start + mid].to_string();
                    let full_end = start + mid + end + 1;
                    s.replace_range(start..full_end, &link_text);
                    continue;
                }
            }
            break;
        }
        // Strip bullet markers.
        let s = s.trim_start_matches("- ").trim_start_matches("• ");
        if !s.is_empty() {
            out.push_str(s);
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Synthesize `text` to an OGG file ready for Telegram sendVoice.
///
/// Returns `None` if the text is too long or synthesis is skipped.
/// Returns `Some(path)` on success — caller must delete the file after use.
pub fn synthesize(text: &str, lang: &str) -> Option<PathBuf> {
    // Strip markdown before length check and synthesis.
    let clean = strip_markdown(text);
    let text = clean.trim();

    // Skip if text is too long.
    if text.chars().count() > tts_max_chars() {
        return None;
    }
    // Skip empty text.
    if text.is_empty() {
        return None;
    }

    let tmp_dir = std::env::temp_dir().join("claudette-tts");
    let _ = std::fs::create_dir_all(&tmp_dir);

    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    let mp3_path = tmp_dir.join(format!("{id}.mp3"));
    let ogg_path = tmp_dir.join(format!("{id}.ogg"));

    if let Err(e) = synthesize_to_mp3(text, lang, &mp3_path) {
        eprintln!("  tts: {e}");
        return None;
    }

    if let Err(e) = mp3_to_ogg(&mp3_path, &ogg_path) {
        eprintln!("  tts: {e}");
        let _ = std::fs::remove_file(&mp3_path);
        return None;
    }

    let _ = std::fs::remove_file(&mp3_path);
    Some(ogg_path)
}

/// Upload an OGG file to Telegram as a voice message.
pub fn send_voice_message(
    http: &reqwest::blocking::Client,
    base_url: &str,
    chat_id: i64,
    ogg_path: &Path,
) -> Result<(), String> {
    let file_bytes =
        std::fs::read(ogg_path).map_err(|e| format!("tts: reading ogg failed: {e}"))?;

    let form = reqwest::blocking::multipart::Form::new()
        .text("chat_id", chat_id.to_string())
        .part(
            "voice",
            reqwest::blocking::multipart::Part::bytes(file_bytes)
                .file_name("voice.ogg")
                .mime_str("audio/ogg")
                .map_err(|e| format!("tts: mime error: {e}"))?,
        );

    let resp = http
        .post(format!("{base_url}/sendVoice"))
        .multipart(form)
        .send()
        .map_err(|e| format!("tts: sendVoice request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("tts: Telegram sendVoice error: {body}"));
    }

    Ok(())
}

/// Check whether TTS dependencies are available.
pub fn check_tts_deps() -> Result<(), String> {
    let output = Command::new("python")
        .args(["-m", "edge_tts", "--help"])
        .output();

    match output {
        Ok(o) if o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty() => Ok(()),
        _ => Err("edge-tts not found. Install with: pip install edge-tts".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_for_lang_english() {
        assert_eq!(voice_for_lang("en"), "en-US-AvaNeural");
    }

    #[test]
    fn voice_for_lang_hebrew() {
        assert_eq!(voice_for_lang("he"), "he-IL-HilaNeural");
    }

    #[test]
    fn voice_for_lang_fallback() {
        assert_eq!(voice_for_lang("fr"), "en-US-AvaNeural");
    }

    #[test]
    fn tts_max_chars_default() {
        let prev = std::env::var("CLAUDETTE_TTS_MAX_CHARS").ok();
        std::env::remove_var("CLAUDETTE_TTS_MAX_CHARS");
        assert_eq!(tts_max_chars(), 500);
        if let Some(v) = prev {
            std::env::set_var("CLAUDETTE_TTS_MAX_CHARS", v);
        }
    }

    #[test]
    fn synthesize_skips_long_text() {
        // 600-char text should return None without hitting the network.
        let long = "a".repeat(600);
        let result = synthesize(&long, "en");
        assert!(result.is_none(), "should skip text over max_chars");
    }

    #[test]
    fn synthesize_skips_empty_text() {
        let result = synthesize("   ", "en");
        assert!(result.is_none());
    }
}
