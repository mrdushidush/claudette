//! One-off diagnostic: chase the gap between the modelled ~200-token
//! turn-1 baseline and the measured 833 input tokens (real session,
//! 2026-05-04). Prints char counts per section + dumps the full
//! payload. Not part of the app — a measurement tool.
//!
//! Run with: `cargo run --example measure_baseline`

use std::fs;

use serde_json::{json, Value};

fn approx_tokens(chars: usize) -> usize {
    // 4 chars/token is the qwen-family BPE rough rule. Order-of-magnitude only.
    (chars + 3) / 4
}

fn print_section(label: &str, content: &str) {
    let chars = content.chars().count();
    let bytes = content.len();
    println!(
        "  {:<28} {:>6} chars  {:>6} bytes  ~{:>4} tokens",
        label,
        chars,
        bytes,
        approx_tokens(chars)
    );
}

fn main() {
    println!("=== claudette turn-1 payload breakdown ===\n");
    println!("cwd: {}", std::env::current_dir().unwrap().display());
    println!("(secretary_system_prompt walks ancestor chain from here)\n");

    // 1. Secretary system prompt — no memory, not concise (REPL/TUI default).
    let prompt_vec = claudette::secretary_system_prompt();
    let prompt = prompt_vec.join("\n\n");
    println!("--- system prompt (concatenated) ---");
    print_section("system_prompt total", &prompt);
    println!();

    // Pull the major chunks back out so we can attribute the bulk.
    // The base + group_hint sentence is everything up to the first "\n\n".
    let mut parts = prompt.splitn(4, "\n\n");
    let base_with_groups = parts.next().unwrap_or("");
    print_section("  base + group hint", base_with_groups);
    let rest: Vec<&str> = parts.collect();
    for (i, chunk) in rest.iter().enumerate() {
        let head: String = chunk.chars().take(60).collect();
        let label = format!("  block #{} [{}…]", i + 1, head.replace('\n', "⏎"));
        print_section(&label, chunk);
    }
    println!();

    // 2. Tool array — what registry ships on turn 1 (no groups enabled).
    let registry = claudette::tool_groups::ToolRegistry::new();
    let tools_value = registry.current_tools();
    let tools_json = serde_json::to_string(&tools_value).unwrap();
    println!("--- tools array (turn 1, no groups enabled) ---");
    print_section("tools_json total", &tools_json);
    if let Some(arr) = tools_value.as_array() {
        for tool in arr {
            let name = tool
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let s = serde_json::to_string(tool).unwrap();
            print_section(&format!("  tool: {}", name), &s);
        }
    }
    println!();

    // 3. Construct the body the way OllamaApiClient::build_request_body would
    //    for a "hey" turn (Ollama path, not openai_compat — that's what the
    //    user's local Ollama-style flow runs).
    let body = json!({
        "model": "qwen3.6:35b-a3b",
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user",   "content": "hey" }
        ],
        "tools": tools_value,
        "stream": true,
        "think": false,
        "options": { "temperature": 0.0, "num_ctx": 32768, "num_predict": 1024 }
    });
    let body_str = serde_json::to_string_pretty(&body).unwrap();
    println!("--- full request body ---");
    print_section("body total (pretty)", &body_str);
    let body_compact = serde_json::to_string(&body).unwrap();
    print_section("body total (compact)", &body_compact);
    println!();

    // 4. Dump for offline inspection / proper tokenization.
    let out_dir = std::env::temp_dir().join("claudette_baseline");
    let _ = fs::create_dir_all(&out_dir);
    let body_path = out_dir.join("turn1_body.json");
    let prompt_path = out_dir.join("turn1_system_prompt.txt");
    let tools_path = out_dir.join("turn1_tools.json");
    fs::write(&body_path, &body_compact).unwrap();
    fs::write(&prompt_path, &prompt).unwrap();
    fs::write(&tools_path, &tools_json).unwrap();
    println!("dumped:");
    println!("  {}", body_path.display());
    println!("  {}", prompt_path.display());
    println!("  {}", tools_path.display());
}
