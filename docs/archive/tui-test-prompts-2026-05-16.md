# Claudette TUI — 50-prompt feature sweep (2026-05-16)

Type each prompt at the TUI input. The **Expect** line is what should appear / what
to watch for. Pause between sections if you want to inspect state.

Recommended setup: start fresh — `claudette --tui` from a clean shell, qwen3.6 brain.

---

## §1 — Core slash dispatcher (1–10)

1. `/help`
   **Expect:** full slash-command list including the 23 we shipped (help, clear, compact, sessions, save, load, status, cost, tools, model, memory, reload, capabilities, validate, agents, preset, brain, coder, models, recall, brownfield, forge, exit).

2. `/status`
   **Expect:** model name, ctx window, token totals, turn count. No brain call.

3. `/models`
   **Expect:** brain + fallback + coder rows; "qwen3.6-35b-a3b" everywhere per the standard.

4. `/tools`
   **Expect:** tool-group summary (file, search, git, advanced, github, recall, facts, todos, calendar, telegram, mission, plus core).

5. `/capabilities`
   **Expect:** backend (LM Studio vs Ollama vs OpenAI), model context size, vision flag, embed flag.

6. `/memory`
   **Expect:** dump of the loaded CLAUDETTE.md memory (under MAX_MEMORY_CHARS).

7. `/sessions`
   **Expect:** list of saved sessions in `~/.claudette/sessions/`. None if fresh.

8. `/save tui-sweep-001`
   **Expect:** "saved" confirmation, file written under sessions dir.

9. `/sessions`
   **Expect:** now includes `tui-sweep-001`.

10. `/cost`
    **Expect:** cumulative input/output tokens for this REPL run.

---

## §2 — Preset & model switching (11–15)

11. `/preset fast`
    **Expect:** swaps to fast preset bundle, rebuild banner.

12. `/preset smart`
    **Expect:** swaps to smart preset; `/models` should now reflect it.

13. `/brain qwen3.6-35b-a3b`
    **Expect:** pins brain, disables auto-fallback. Banner.

14. `/brain auto`
    **Expect:** re-enables current preset's fallback.

15. `/coder qwen3.6-35b-a3b`
    **Expect:** sets the codet coder; persists for the process only.

---

## §3 — Brain natural-language, no tools (16–20)

16. `What is 17 * 23, and explain the steps you used.`
    **Expect:** 391. Watch for `add_numbers` not being called (removed from schema).

17. `In one paragraph, explain how a bloom filter differs from a hash set.`
    **Expect:** clean prose, no tool calls.

18. `List five idioms in Rust that surprise people coming from Python.`
    **Expect:** 5 items, no hallucinated stdlib names.

19. `What time is it right now?`
    **Expect:** **calls `get_current_time` tool** — confirms tool-routing still fires for trivial asks.

20. `Without using any tools, summarize what you remember about this project.`
    **Expect:** mentions claudette / forge / qwen3.6 from CLAUDETTE.md context.

---

## §4 — Tool use: files / search / git / todos / facts (21–28)

21. `Create a file at scratch/hello.txt containing the single line "hello from tui sweep" and confirm it exists.`
    **Expect:** file tool writes, then reads back to verify. Check disk.

22. `Read the first 30 lines of crates/claudette/src/commands.rs and tell me what SlashCommand variants exist.`
    **Expect:** uses file/read or search; enumerates Help, Clear, Compact, Sessions, etc.

23. `Grep the repo for "RecallReprobe" and tell me which files reference it.`
    **Expect:** search tool returns ≥2 hits (commands.rs and at least one caller).

24. `What's the current git status?`
    **Expect:** git tool reports `?? launch-drafts/` and any new files we just wrote.

25. `Show me the last 3 commits on this branch.`
    **Expect:** git log tool returns 9c886b7, d5d105a, f785a32 (or newer).

26. `Add a todo item: "verify MTP repoint after sweep" with priority high.`
    **Expect:** todos tool stores it; confirm via "list my todos".

27. `Save a fact: my GPU is RTX 5060 Ti 16GB and runs qwen3.6 at ~24 tok/s baseline / ~43 tok/s with MTP.`
    **Expect:** facts tool persists; later `/recall` queries should find it.

28. `What's on my calendar today and tomorrow?`
    **Expect:** calendar tool (or graceful "no calendar configured" if not wired).

---

## §5 — Recall & cross-session memory (29–33)

29. `/save tui-sweep-mid`
    **Expect:** snapshot saved.

30. `/clear`
    **Expect:** runtime rebuilt; turn count resets.

31. `/recall MTP benchmark`
    **Expect:** hits the embed-indexed memory; returns lines from prior sessions about the 1.77× speedup.

32. `/recall reprobe`
    **Expect:** clears RECALL_INDEX_BROKEN, re-runs embed probe, prints result. Should be "ok" if embed model is loaded.

33. `What do I have facts saved about my GPU?`
    **Expect:** brain uses recall/facts tool and surfaces the RTX 5060 Ti fact from prompt 27.

---

## §6 — Codet / code generation (34–38)

34. `Write a tiny Rust function fn slugify(input: &str) -> String that lowercases and replaces runs of non-alphanumeric chars with a single dash. Return just the function and one unit test.`
    **Expect:** codet path or brain-direct; compiles in isolation.

35. `Refactor the slugify function so it returns Cow<'_, str> when no allocation is needed.`
    **Expect:** correct use of Cow::Borrowed when input already valid.

36. `Write a PowerShell one-liner that finds all .rs files modified in the last 7 days under crates/.`
    **Expect:** uses `Get-ChildItem -Recurse` + `LastWriteTime` comparison; no `find` (Unix).

37. `Generate a small TypeScript zod schema for a User with id (uuid), email, and createdAt (iso8601).`
    **Expect:** clean z.object(...) with z.string().uuid() etc.

38. `Now ship the same User shape as a Python pydantic v2 model.`
    **Expect:** BaseModel, EmailStr, datetime, model_config not config.

---

## §7 — Forge / brownfield / mission (39–44)

> Forge needs an active mission. We'll use a throwaway target.

39. `/brownfield owner/example-tiny-repo`
    **Expect:** mission_start tool runs; clone path printed; .git/info/exclude updated with the marker filename (per mission-marker-leak fix).

40. `/status`
    **Expect:** now shows active mission with target + marker.

41. `/forge add a README.md with a one-line description "tiny repo used for claudette TUI sweep"`
    **Expect:** Planner → Coder → Verifier loop. Watch the reload classifier (c6c5969 fix) — should NOT thrash. Verifier diffs against base SHA.

42. `Run forge again with a smaller ask: /forge add a .gitignore that ignores target/`
    **Expect:** second round-trip; mission_submit refuses if tree is clean.

43. `What's the diff currently staged in the mission worktree?`
    **Expect:** git tool inside the mission dir; lists the README/.gitignore changes.

44. `Finalize and submit the mission with a PR titled "claudette tui sweep test".`
    **Expect:** mission_submit fires; if no GH remote, graceful error (not a panic).

---

## §8 — Compaction / context stress (45–47)

45. `Paste the entire content of crates/claudette/src/commands.rs into the chat and summarize it.`
    **Expect:** large input → compaction may trigger; tier-aware log; no model-reload panic (post-v0.4.0 fix). If model reload is needed, the 750ms backoff retry should kick in.

46. `/compact`
    **Expect:** explicit compaction; CompactionOutcome logs the tier; conversation shrinks.

47. `/status`
    **Expect:** token totals lower than pre-compact; turn count preserved.

---

## §9 — Edge cases & robustness (48–50)

48. `/validate ~/.claudette/files/userClass.py` (or any path you don't have)
    **Expect:** graceful "file not found" or validation report; no panic.

49. (Attach an image — drag a PNG/JPG into TUI) `What's in this image?`
    **Expect:** vision tool fires if model supports it; otherwise graceful refusal. Compaction should evict older image bytes if multiple images sent (per evict_older_image_bytes fix).

50. `/load tui-sweep-001` then `/exit`
    **Expect:** restored to the §1 starting state (cumulative tokens preserved if `ReplState` survives); `/exit` cleanly closes the TUI.

---

## Quick-look findings template

For each surface that misbehaves, jot:

- **Prompt #:**
- **Symptom:**
- **Expected vs got:**
- **Logs (line/file if obvious):**

If a finding looks systemic (≥2 prompts), promote it to a memory entry after the sweep.
