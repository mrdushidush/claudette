//! Vision group — `screenshot_capture` + `image_describe`. Sprint v0.6.0
//! Phase 3.2b. Both tools are best-effort and degrade with clear,
//! actionable errors when the required system bits aren't in place.
//!
//! `screenshot_capture` shells out to a platform-native screenshot
//! command (PowerShell + System.Drawing on Windows, `screencapture` on
//! macOS, `gnome-screenshot` / `import` fallback on Linux) so we don't
//! pull in a vision crate just for one tool. Output lands at
//! `~/.claudette/files/screenshot-<ts>.png`.
//!
//! `image_describe` POSTs to LM Studio's OpenAI-compatible
//! `/v1/chat/completions` with the image attached as a `data:`-URL
//! `image_url` content block. Returns the assistant's text or a
//! friendly error pointing at `docs/vision.md` if no VLM is loaded.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{claudette_home, ensure_dir, parse_json_input};
use crate::image_attach::{encode_base64_standard, image_mime_from_path};
use crate::test_runner::run_command_with_timeout;

const SHOT_TIMEOUT_SECS: u64 = 15;
const DESCRIBE_TIMEOUT_SECS: u64 = 120;
const DESCRIBE_MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "screenshot_capture",
                "description": "Capture the primary display to a PNG under ~/.claudette/files/. Returns {path}. Uses PowerShell on Windows, screencapture on macOS, gnome-screenshot/import on Linux.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "image_describe",
                "description": "Ask a vision-language model loaded in LM Studio to describe an image (PNG/JPG/GIF/WebP/BMP). Returns {description}. Requires a VLM (e.g. Qwen2.5-VL) loaded at LMS_API_URL/v1.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":     { "type": "string", "description": "Image path (absolute or ~/)." },
                        "question": { "type": "string", "description": "Optional question; default 'Describe this image.'" }
                    },
                    "required": ["path"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "screenshot_capture" => run_screenshot_capture(input),
        "image_describe" => run_image_describe(input),
        _ => return None,
    };
    Some(result)
}

fn screenshots_dir() -> PathBuf {
    claudette_home().join("files")
}

fn run_screenshot_capture(_input: &str) -> Result<String, String> {
    ensure_dir(&screenshots_dir())?;
    let ts = chrono::Local::now().format("%Y%m%dT%H%M%S").to_string();
    let path = screenshots_dir().join(format!("screenshot-{ts}.png"));
    let path_str = path.display().to_string();

    let (program, args, args_owned) = build_screenshot_command(&path_str);
    let _ = args_owned; // keep the temporary string alive
    let result = run_command_with_timeout(program, &args, SHOT_TIMEOUT_SECS, None);
    if !result.success {
        return Err(format!(
            "screenshot_capture: command failed (exit {:?}): {} {}",
            result.exit_code,
            result
                .stderr
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | "),
            result
                .stdout
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | "),
        ));
    }
    if !path.exists() {
        return Err(format!(
            "screenshot_capture: command reported success but {} doesn't exist",
            path.display()
        ));
    }

    Ok(json!({
        "ok": true,
        "path": path_str,
    })
    .to_string())
}

/// Build the platform-native screenshot command. The returned String is
/// referenced inside `args` via &str, so the caller holds it alive until
/// after run_command_with_timeout has consumed args.
#[cfg(target_os = "windows")]
fn build_screenshot_command(out_path: &str) -> (&'static str, Vec<&str>, String) {
    let escaped = out_path.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
         $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
         $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height; \
         $g = [System.Drawing.Graphics]::FromImage($bmp); \
         $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size); \
         $bmp.Save('{escaped}', [System.Drawing.Imaging.ImageFormat]::Png); \
         $g.Dispose(); $bmp.Dispose()"
    );
    // We have to return `args` as Vec<&str>; allocate a leaked Box so the
    // PowerShell string outlives the call. (Test surface is small; leaking
    // a few hundred bytes per call won't matter at agent-call cadence.)
    let leaked: &'static str = Box::leak(script.into_boxed_str());
    (
        "powershell",
        vec!["-NoProfile", "-NonInteractive", "-Command", leaked],
        String::new(),
    )
}

#[cfg(target_os = "macos")]
fn build_screenshot_command(out_path: &str) -> (&'static str, Vec<&str>, String) {
    let leaked: &'static str = Box::leak(out_path.to_string().into_boxed_str());
    ("screencapture", vec!["-x", leaked], String::new())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn build_screenshot_command(out_path: &str) -> (&'static str, Vec<&str>, String) {
    // Linux fallback — prefer gnome-screenshot, fall back to imagemagick
    // import. The wrapper runs whichever is on PATH via `sh -c`.
    let escaped = out_path.replace('\'', "'\\''");
    let script = format!(
        "if command -v gnome-screenshot >/dev/null 2>&1; then \
            gnome-screenshot -f '{escaped}'; \
         elif command -v import >/dev/null 2>&1; then \
            import -window root '{escaped}'; \
         elif command -v scrot >/dev/null 2>&1; then \
            scrot '{escaped}'; \
         else \
            echo 'no screenshot tool on PATH (install gnome-screenshot, imagemagick, or scrot)'; \
            exit 1; \
         fi"
    );
    let leaked: &'static str = Box::leak(script.into_boxed_str());
    ("sh", vec!["-c", leaked], String::new())
}

fn run_image_describe(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "image_describe")?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("image_describe: missing 'path'")?;
    let question = v
        .get("question")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("Describe this image.");

    // Resolve `~/` and validate it lands under home (same gate as edit_file
    // — the user may have explicit screenshots elsewhere but we don't want
    // to expose arbitrary system images by mistake).
    let path = super::validate_read_path(path_str)?;

    let mime = image_mime_from_path(&path).ok_or_else(|| {
        format!(
            "image_describe: '{}' is not a supported image type (PNG/JPG/GIF/WebP/BMP)",
            path.display()
        )
    })?;
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("image_describe: read {} failed: {e}", path.display()))?;
    if bytes.len() > DESCRIBE_MAX_IMAGE_BYTES {
        return Err(format!(
            "image_describe: {} is {} bytes, exceeds {}",
            path.display(),
            bytes.len(),
            DESCRIBE_MAX_IMAGE_BYTES
        ));
    }
    let data_url = format!("data:{mime};base64,{}", encode_base64_standard(&bytes));

    let base = std::env::var("LMS_API_URL")
        .or_else(|_| std::env::var("OLLAMA_HOST"))
        .unwrap_or_else(|_| "http://localhost:1234".to_string());
    let model = std::env::var("CLAUDETTE_VISION_MODEL").unwrap_or_else(|_| "vision".to_string());
    let url = format!("{base}/v1/chat/completions");

    let payload = json!({
        "model": model,
        "max_tokens": 512,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": question },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]
        }]
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(DESCRIBE_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("image_describe: build client failed: {e}"))?;
    let resp = client.post(&url).json(&payload).send().map_err(|e| {
        format!(
            "image_describe: cannot reach {url} ({e}). Is LM Studio running with a vision \
                 model loaded? See docs/vision.md."
        )
    })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "image_describe: HTTP {status}: {} \
             — confirm a vision-language model (Qwen2.5-VL, InternVL, etc.) is loaded.",
            text.chars().take(400).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("image_describe: parse failed: {e}"))?;
    let description = data
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Ok(json!({
        "ok": true,
        "model": model,
        "description": description,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_two_tools() {
        let s = schemas();
        assert_eq!(s.len(), 2);
        let names: Vec<&str> = s
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["screenshot_capture", "image_describe"]);
    }

    #[test]
    fn image_describe_rejects_missing_path() {
        let err = run_image_describe("{}").unwrap_err();
        assert!(err.contains("missing 'path'"), "got: {err}");
    }

    #[test]
    fn image_describe_rejects_non_image_extension() {
        // Use the shared temp-HOME helper rather than reading the ambient
        // HOME/USERPROFILE directly. `validate_read_path` (called inside
        // `run_image_describe`) re-resolves the home dir, so reading the
        // global env here would race any concurrent test that swaps HOME —
        // the path we built would fall outside the home seen at dispatch and
        // fail the read-guard *before* the extension check we're asserting on.
        // `with_temp_home` pins HOME to a private temp dir AND holds the
        // process-wide env lock for the whole closure, killing the race. The
        // file lives under that home so it survives the read-guard; the
        // unsupported-extension check then fires as intended.
        crate::with_temp_home(|home| {
            let path = home.join("claudette-not-an-image-xyz.txt");
            let _ = std::fs::write(&path, "hi");
            let err = run_image_describe(&json!({ "path": path.to_string_lossy() }).to_string())
                .unwrap_err();
            assert!(
                err.contains("not a supported image type") || err.contains("missing"),
                "got: {err}"
            );
        });
    }

    #[test]
    fn build_screenshot_command_returns_non_empty_program() {
        let (program, args, _kept) = build_screenshot_command("/tmp/test.png");
        assert!(!program.is_empty());
        assert!(!args.is_empty());
    }
}
