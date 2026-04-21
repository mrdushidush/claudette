//! Morning-briefing prompt (Phase 3 of the life-agent sprint).
//!
//! The same prompt text is used by both the `/briefing` Telegram slash
//! command (ad-hoc on-demand briefings) and the scheduled recurring
//! briefing created by `claudette --briefing`. Keeping it in one place
//! ensures the two paths produce the same style of output and lets phase
//! 4's Gmail extension grow this without touching two call sites.

/// Instructions handed to the model at turn time. Written as a user-turn
/// prompt (not a system addendum) because the existing runtime has a
/// fixed system prompt per brain preset and the 4b/9b Qwen brains follow
/// inline instructions well.
pub const BRIEFING_PROMPT: &str = "\
Give me my morning briefing.

Step 1: Call enable_tools(\"facts\") and enable_tools(\"calendar\") if those \
groups aren't already loaded.

Step 2: Gather the facts:
- get_current_time to anchor the day/date.
- weather_current — pass a location if one is set in the conversation \
memory, otherwise skip the weather line.
- calendar_list_events for today (time_min=now, time_max=end of today in \
the local timezone).

Step 3: Reply with plain text, under 200 words, no greeting, no sign-off, \
no markdown headers. Structure:
- One line: day of week + date.
- One line: weather (or skip if unavailable).
- Events: up to 5 items as '- HH:MM  Event title' (or 'No calendar events \
today.' if empty).
- One final encouragement or concrete suggestion for the first block of \
the day, max one sentence.";
