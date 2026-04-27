"""
Probe LM Studio's /v1/chat/completions with claudette's exact iter-2 shape
and several variants, looking for the variant that doesn't trigger
"No user query found in messages." or fake-sandbox refusals.

Run: python tests/probe_iter2_shape.py
Requires: LM Studio loaded with qwen3.5-4b, port 1234.
"""
import json, urllib.request, urllib.error, copy

ENDPOINT = "http://localhost:1234/v1/chat/completions"
MODEL = "qwen3.5-4b"

# Claudette's actual system prompt is large; this approximates with the
# load-bearing parts that the model reasons about (tool list reference and
# the operator persona).
SYSTEM = (
    "You are claudette, a local-first AI coding agent. You have access to "
    "tools listed below. When the user asks for files or directory contents, "
    "use list_dir. When the user asks to search code, use grep_search. "
    "Always use the tools — never refuse based on an imagined sandbox."
)

USER = "What files are in D:/dev/claudette/src?"

# Synthetic tool result the way claudette would see it after a successful
# list_dir call: a short directory listing.
TOOL_RESULT = (
    "agents.rs\napi.rs\nmain.rs\nrun.rs\ntools.rs\ntheme.rs\nlib.rs"
)

TOOL_CALL_ID = "call_abc123"

TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "list_dir",
            "description": "List the contents of a directory.",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "grep_search",
            "description": "Search for a pattern in files.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                },
                "required": ["pattern"],
            },
        },
    },
]

# V1 — claudette's exact iter-2 shape (the suspect)
V1 = [
    {"role": "system", "content": SYSTEM},
    {"role": "user", "content": USER},
    {"role": "assistant", "content": None, "tool_calls": [
        {"id": TOOL_CALL_ID, "type": "function",
         "function": {"name": "list_dir",
                      "arguments": '{"path":"D:/dev/claudette/src"}'}}
    ]},
    {"role": "tool", "tool_call_id": TOOL_CALL_ID, "content": TOOL_RESULT},
]

# V2 — same but assistant.content = "" instead of null
V2 = copy.deepcopy(V1)
V2[2]["content"] = ""

# V3 — append a synthetic user "continue" after the tool result
V3 = V1 + [{"role": "user", "content": "Please answer based on those results."}]

# V4 — drop the assistant tool_calls + tool roles entirely; replace with a
# user message embedding the tool result inline ("naive replay" form)
V4 = [
    {"role": "system", "content": SYSTEM},
    {"role": "user", "content": USER},
    {"role": "user", "content": (
        "I ran list_dir for you. Result:\n" + TOOL_RESULT +
        "\nNow answer the original question."
    )},
]

# V5 — V1 + chat_template_kwargs enable_thinking=false at request level
V5_BODY_EXTRA = {"chat_template_kwargs": {"enable_thinking": False}}

# V6 — V1 + drop tools array from request body (only the messages reference
# tool calls; no top-level tools schema)
V6_NO_TOOLS = True

VARIANTS = [
    ("V1_claudette_iter2_baseline", V1, {}, False),
    ("V2_assistant_empty_string",   V2, {}, False),
    ("V3_synthetic_user_continue",  V3, {}, False),
    ("V4_naive_replay_as_user",     V4, {}, False),
    ("V5_enable_thinking_false",    V1, V5_BODY_EXTRA, False),
    ("V6_no_tools_in_request",      V1, {}, True),
]


def call(name, messages, body_extra, no_tools):
    body = {
        "model": MODEL,
        "messages": messages,
        "stream": False,
        "temperature": 0.0,
        "max_tokens": 256,
    }
    if not no_tools:
        body["tools"] = TOOLS
    body.update(body_extra)

    req = urllib.request.Request(
        ENDPOINT,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    print("=" * 70)
    print(f"VARIANT: {name}")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            choice = data["choices"][0]["message"]
            content = choice.get("content")
            tool_calls = choice.get("tool_calls")
            finish = data["choices"][0].get("finish_reason")
            print(f"  HTTP 200 — finish_reason={finish}")
            if content:
                preview = content[:280].replace("\n", "\\n")
                print(f"  content: {preview}")
            if tool_calls:
                for tc in tool_calls:
                    print(f"  tool_call: {tc['function']['name']} args={tc['function']['arguments'][:120]}")
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        print(f"  HTTP {e.code} — {body[:400]}")
    except Exception as e:
        print(f"  EXCEPTION: {e}")


if __name__ == "__main__":
    for name, msgs, extra, no_tools in VARIANTS:
        call(name, msgs, extra, no_tools)
    print("=" * 70)
