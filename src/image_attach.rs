//! Shared helpers for turning user input into image attachments — used
//! by both the TUI (Alt+V clipboard, bracketed-paste drop) and the REPL
//! (drag-drop path detection on submit).
//!
//! The TUI also reads the OS clipboard via `arboard`; that lives in
//! `tui.rs` because it's hotkey-driven and only meaningful in raw mode.
//! Everything that's reusable across both modes lives here.

use crate::tui_events::ImageAttachment;

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Standard-alphabet base64 encoder. Hand-rolled to match the
/// hand-rolled base64url decoder in `tools/gmail.rs` — keeps the
/// dependency surface unchanged for what is otherwise a 25-line routine.
#[must_use]
pub fn encode_base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (b0, b1, b2, len) = match chunk {
            [a] => (*a, 0u8, 0u8, 1usize),
            [a, b] => (*a, *b, 0u8, 2),
            [a, b, c] => (*a, *b, *c, 3),
            _ => unreachable!(),
        };
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        if len >= 2 {
            out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if len >= 3 {
            out.push(ALPHABET[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Return the IANA MIME type for a path's extension, if it looks like a
/// supported image format.
#[must_use]
pub fn image_mime_from_path(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

/// Read a file from disk and turn it into an `ImageAttachment`. Caps the
/// payload at 20 MiB so a stray `@/path/to/dvd.iso` typo doesn't load
/// gigabytes into the session.
pub fn attachment_from_file(path: &std::path::Path) -> Result<ImageAttachment, String> {
    let mime = image_mime_from_path(path)
        .ok_or_else(|| format!("not an image file: {}", path.display()))?;
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("can't stat {}: {e}", path.display()))?;
    if metadata.len() as usize > MAX_IMAGE_BYTES {
        return Err(format!(
            "image too large ({} bytes, cap is {MAX_IMAGE_BYTES}): {}",
            metadata.len(),
            path.display()
        ));
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    Ok(ImageAttachment {
        media_type: mime.to_string(),
        data_b64: encode_base64_standard(&bytes),
    })
}

/// Tokenise a single input line into shell-ish whitespace-separated
/// tokens, honouring double-quoted segments so drag-dropped Windows
/// paths with spaces (e.g. `"C:\\Users\\me\\My Pictures\\foo.png"`)
/// survive as one token.
#[must_use]
pub fn split_path_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Outcome of scanning a submitted input line for image-path tokens.
/// Diagnostic counts let callers show "📎 N attached" / "1 path detected
/// but unreadable" instead of silently dropping mismatches.
pub struct ExtractResult {
    pub attached: Vec<ImageAttachment>,
    pub extension_matches: usize,
    pub first_failure: Option<String>,
}

/// Walk `input` for tokens that look like image-file paths — `@`-prefixed,
/// quoted, or bare paths with a recognised image extension — and turn the
/// readable ones into `ImageAttachment`s. The token strings stay in the
/// caller's `input` verbatim so the assistant can still see what the user
/// referenced.
#[must_use]
pub fn extract_image_attachments_from_input(input: &str) -> ExtractResult {
    let mut attached = Vec::new();
    let mut extension_matches = 0usize;
    let mut first_failure: Option<String> = None;
    for token in split_path_tokens(input) {
        let candidate = token
            .strip_prefix('@')
            .unwrap_or(token.as_str())
            .trim_matches('"');
        let path = std::path::Path::new(candidate);
        if image_mime_from_path(path).is_none() {
            continue;
        }
        extension_matches += 1;
        if !path.is_file() {
            if first_failure.is_none() {
                first_failure = Some(format!("not a file: {}", path.display()));
            }
            continue;
        }
        match attachment_from_file(path) {
            Ok(att) => attached.push(att),
            Err(e) => {
                if first_failure.is_none() {
                    first_failure = Some(e);
                }
            }
        }
    }
    ExtractResult {
        attached,
        extension_matches,
        first_failure,
    }
}
