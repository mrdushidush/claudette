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
///
/// Phase 4 additions: Gmail read-only. If the user has authenticated
/// Gmail (`claudette --auth-google gmail`), we include an unread VIP
/// summary. If not, the gmail_* tools return an auth error and the
/// model is told to skip the email line silently — a missing section
/// is better than a visible failure.
pub const BRIEFING_PROMPT: &str = "\
Give me my morning briefing.

Step 1: Load tool groups you'll need. Call enable_tools(\"facts\"), \
enable_tools(\"calendar\"), and enable_tools(\"gmail\") in sequence. \
Each one only needs to be called if the group isn't already loaded.

Step 2: Gather the facts:
- get_current_time to anchor the day/date.
- weather_current — pass a location if one is set in the conversation \
memory, otherwise skip the weather line.
- calendar_list_events for today (time_min=now, time_max=end of today in \
the local timezone).
- gmail_list with query='is:unread newer_than:1d' and max_results=10 to \
see unread mail. If this call returns an authentication error, silently \
skip the email line — do NOT mention the error to the user.

Step 3: Reply with plain text, under 200 words, no greeting, no sign-off, \
no markdown headers. Structure:
- One line: day of week + date.
- One line: weather (or skip if unavailable).
- Events: up to 5 items as '- HH:MM  Event title' (or 'No calendar events \
today.' if empty).
- Email summary: 'N unread' with up to 3 notable subjects (prefer senders \
that look like real people over newsletters). Skip entirely if gmail auth \
is absent.
- One final encouragement or concrete suggestion for the first block of \
the day, max one sentence.";
