//! Persistent scheduler (AD-4 from docs/sprint_life_agent.md).
//!
//! Single owner of `~/.claudette/schedule.jsonl`. Maintains an in-memory
//! list of [`ScheduleEntry`] records and fires them when their `next_fire_at`
//! has passed. Natural-language expressions are parsed deterministically in
//! Rust via [`parse_expression`]; the LLM never computes wall-clock time
//! itself — it only proposes a string, and the parser either validates it
//! or returns a structured error the model can retry against.
//!
//! Concurrency: this module exposes a plain struct, not a thread. The
//! Telegram consumer owns a [`Scheduler`] and calls [`Scheduler::fire_due`]
//! on its event loop; a thin background thread elsewhere wakes the
//! consumer up when `next_due_at()` passes. See AD-1.

use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};

use crate::clock::{Clock, SystemClock};

/// Cap on per-restart catch-up firings when catch_up == "all". Prevents a
/// bot that was off for a year from pinging the chat 8760 times for an
/// hourly reminder.
const MAX_CATCH_UP_ALL: usize = 50;

/// Kind of schedule — stored in the jsonl for forward-compat when we add
/// more variants (e.g. "until" expiry).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScheduleKind {
    OneShot,
    Recurring,
}

/// What to do about occurrences the bot missed while it was off.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CatchUp {
    /// Fire once on restart if we missed at least one occurrence; drop the
    /// rest. Default for reminders.
    #[default]
    Once,
    /// Silently skip missed occurrences. Default for briefings — a 7 am
    /// briefing seen at 9 am is spam.
    Skip,
    /// Fire every missed occurrence. Rare, only useful for logging/audit
    /// cases. Capped at [`MAX_CATCH_UP_ALL`] per restart.
    All,
}

/// One persisted schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub id: String,
    pub kind: ScheduleKind,
    pub original_expr: String,
    pub next_fire_at: DateTime<Utc>,
    /// Standard 7-field cron expression (`sec min hour dom mon dow year`).
    /// `None` for one-shot entries.
    pub recurrence: Option<String>,
    pub prompt: String,
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub catch_up: CatchUp,
    pub created_at: DateTime<Utc>,
}

/// Parse result before the entry is materialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSchedule {
    OneShot { at: DateTime<Utc> },
    Recurring { cron: String, next_fire_at: DateTime<Utc> },
}

/// A single firing ready to deliver to the consumer.
#[derive(Debug, Clone)]
pub struct Firing {
    pub entry_id: String,
    pub prompt: String,
    pub chat_id: Option<i64>,
    pub scheduled_for: DateTime<Utc>,
}

// ──────────────────────────────────────────────────────────────────────────
// Parser
// ──────────────────────────────────────────────────────────────────────────

/// Parse a natural-language schedule expression. Returns either a one-shot
/// `DateTime<Utc>` or a recurring cron string + its next fire time.
///
/// Supported forms (case-insensitive, extra whitespace OK):
///   - `in 30 minutes` / `in 2 hours` / `in 1 day`
///   - `today at 15:00` / `today at 3pm`
///   - `tomorrow at 09:30` / `tomorrow at 9am`
///   - `at 07:00` — next future occurrence (today if still ahead, else tomorrow)
///   - `<RFC3339 datetime>` — e.g. `2026-04-22T15:00:00-04:00`
///   - `every day at HH:MM` / `daily at HH:MM`
///   - `every weekday at HH:MM` / `weekdays at HH:MM`
///   - `every (mon|tue|wed|thu|fri|sat|sun) at HH:MM`
///   - `every N (minutes|hours)` — interval recurrence
///   - `cron: <7-field cron>` — raw cron passthrough for power users
///
/// All times are interpreted in the **user's local timezone** so "at 7am"
/// behaves the way the user expects regardless of where the server runs.
pub fn parse_expression(expr: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let normalised = expr.trim().to_lowercase();
    if normalised.is_empty() {
        return Err("empty schedule expression".to_string());
    }

    // Raw RFC3339 / ISO 8601 — try first so a user can paste a machine date.
    if let Ok(at) = DateTime::parse_from_rfc3339(expr.trim()) {
        return Ok(ParsedSchedule::OneShot {
            at: at.with_timezone(&Utc),
        });
    }

    // Raw cron passthrough.
    if let Some(rest) = normalised.strip_prefix("cron:") {
        let cron = rest.trim().to_string();
        let next = next_cron_fire(&cron, clock)?;
        return Ok(ParsedSchedule::Recurring { cron, next_fire_at: next });
    }

    // "in N (minute|hour|day)s"
    if let Some(rest) = normalised.strip_prefix("in ") {
        return parse_relative(rest, clock);
    }

    // "today at …"
    if let Some(rest) = normalised.strip_prefix("today at ") {
        return parse_today_at(rest, clock);
    }

    // "tomorrow at …"
    if let Some(rest) = normalised.strip_prefix("tomorrow at ") {
        return parse_tomorrow_at(rest, clock);
    }

    // "at …" — next future occurrence.
    if let Some(rest) = normalised.strip_prefix("at ") {
        return parse_bare_at(rest, clock);
    }

    // Recurring forms.
    if let Some(rest) = normalised.strip_prefix("every ") {
        return parse_every(rest, clock);
    }
    if let Some(rest) = normalised.strip_prefix("daily at ") {
        return build_daily(rest, clock);
    }
    if let Some(rest) = normalised.strip_prefix("weekdays at ") {
        return build_weekdays(rest, clock);
    }

    Err(format!(
        "could not parse schedule expression '{expr}'. Try forms like \
         'in 30 minutes', 'tomorrow at 15:00', 'every weekday at 07:00', \
         or 'cron: 0 0 7 * * Mon-Fri *'."
    ))
}

fn parse_relative(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let mut parts = rest.split_whitespace();
    let n_str = parts
        .next()
        .ok_or_else(|| "expected a number after 'in'".to_string())?;
    let unit = parts
        .next()
        .ok_or_else(|| "expected a unit (minute/hour/day) after the number".to_string())?;
    let n: i64 = n_str
        .parse()
        .map_err(|_| format!("not a number: '{n_str}'"))?;
    if n <= 0 {
        return Err(format!("interval must be positive, got {n}"));
    }
    let delta = match unit.trim_end_matches('s') {
        "minute" | "min" | "m" => Duration::minutes(n),
        "hour" | "hr" | "h" => Duration::hours(n),
        "day" | "d" => Duration::days(n),
        "second" | "sec" => Duration::seconds(n),
        other => return Err(format!("unknown time unit '{other}'")),
    };
    Ok(ParsedSchedule::OneShot {
        at: clock.now() + delta,
    })
}

fn parse_today_at(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let t = parse_time_of_day(rest)?;
    let today = local_today(clock);
    let at = local_combine(today, t)?;
    Ok(ParsedSchedule::OneShot { at })
}

fn parse_tomorrow_at(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let t = parse_time_of_day(rest)?;
    let tomorrow = local_today(clock) + Duration::days(1);
    let at = local_combine(tomorrow, t)?;
    Ok(ParsedSchedule::OneShot { at })
}

fn parse_bare_at(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let t = parse_time_of_day(rest)?;
    let today = local_today(clock);
    let candidate = local_combine(today, t)?;
    let at = if candidate > clock.now() {
        candidate
    } else {
        local_combine(today + Duration::days(1), t)?
    };
    Ok(ParsedSchedule::OneShot { at })
}

fn parse_every(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    // "every N minute[s]" / "every N hour[s]"
    let mut parts = rest.split_whitespace();
    let first = parts
        .next()
        .ok_or_else(|| "empty 'every …' expression".to_string())?;

    if let Ok(n) = first.parse::<i64>() {
        if n <= 0 {
            return Err(format!("interval must be positive, got {n}"));
        }
        let unit = parts
            .next()
            .ok_or_else(|| "expected a unit after 'every N'".to_string())?;
        let cron = match unit.trim_end_matches('s') {
            "minute" | "min" | "m" => format!("0 */{n} * * * * *"),
            "hour" | "hr" | "h" => format!("0 0 */{n} * * * *"),
            other => return Err(format!("unsupported 'every N' unit '{other}'")),
        };
        let next_fire_at = next_cron_fire(&cron, clock)?;
        return Ok(ParsedSchedule::Recurring { cron, next_fire_at });
    }

    // "every day at HH:MM" / "every weekday at HH:MM" / "every mon at HH:MM"
    let tail = parts.collect::<Vec<_>>().join(" ");
    let (kind, at_time) = tail
        .split_once(" at ")
        .or_else(|| {
            if first == "day" || first == "weekday" {
                tail.strip_prefix("at ").map(|t| ("", t))
            } else {
                None
            }
        })
        .ok_or_else(|| {
            format!(
                "couldn't parse 'every {rest}'. Try 'every day at HH:MM' or \
                 'every weekday at 07:00'."
            )
        })?;

    let t = parse_time_of_day(at_time)?;
    match first {
        "day" => build_daily_from_time(t, clock),
        "weekday" => build_weekdays_from_time(t, clock),
        day if parse_weekday(day).is_some() => {
            let dow = parse_weekday(day).unwrap();
            build_weekly_from_time(dow, t, clock)
        }
        other => {
            // "every monday at HH:MM" where kind is also captured.
            if let Some(dow) = parse_weekday(other) {
                build_weekly_from_time(dow, t, clock)
            } else if let Some(dow) = parse_weekday(kind) {
                build_weekly_from_time(dow, t, clock)
            } else {
                Err(format!("unsupported 'every {first}' pattern (tail='{tail}')"))
            }
        }
    }
}

fn build_daily(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let t = parse_time_of_day(rest)?;
    build_daily_from_time(t, clock)
}

fn build_weekdays(rest: &str, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let t = parse_time_of_day(rest)?;
    build_weekdays_from_time(t, clock)
}

fn build_daily_from_time(t: NaiveTime, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let cron = format!("0 {m} {h} * * * *", h = t.hour(), m = t.minute());
    let next_fire_at = next_cron_fire(&cron, clock)?;
    Ok(ParsedSchedule::Recurring { cron, next_fire_at })
}

fn build_weekdays_from_time(t: NaiveTime, clock: &dyn Clock) -> Result<ParsedSchedule, String> {
    let cron = format!(
        "0 {m} {h} * * Mon-Fri *",
        h = t.hour(),
        m = t.minute()
    );
    let next_fire_at = next_cron_fire(&cron, clock)?;
    Ok(ParsedSchedule::Recurring { cron, next_fire_at })
}

fn build_weekly_from_time(
    dow: &'static str,
    t: NaiveTime,
    clock: &dyn Clock,
) -> Result<ParsedSchedule, String> {
    let cron = format!(
        "0 {m} {h} * * {dow} *",
        h = t.hour(),
        m = t.minute(),
        dow = dow
    );
    let next_fire_at = next_cron_fire(&cron, clock)?;
    Ok(ParsedSchedule::Recurring { cron, next_fire_at })
}

fn parse_weekday(s: &str) -> Option<&'static str> {
    match s {
        "mon" | "monday" => Some("Mon"),
        "tue" | "tues" | "tuesday" => Some("Tue"),
        "wed" | "weds" | "wednesday" => Some("Wed"),
        "thu" | "thur" | "thurs" | "thursday" => Some("Thu"),
        "fri" | "friday" => Some("Fri"),
        "sat" | "saturday" => Some("Sat"),
        "sun" | "sunday" => Some("Sun"),
        _ => None,
    }
}

/// Parse `HH:MM`, `HH`, `H:MM am/pm`, `H am/pm` into a `NaiveTime`.
fn parse_time_of_day(s: &str) -> Result<NaiveTime, String> {
    let s = s.trim();
    // am/pm suffix
    let (body, ampm) = if let Some(b) = s.strip_suffix("am") {
        (b.trim(), Some(false))
    } else if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else {
        (s, None)
    };

    let (hour_s, minute_s) = match body.split_once(':') {
        Some((h, m)) => (h, m),
        None => (body, "00"),
    };
    let mut hour: u32 = hour_s
        .parse()
        .map_err(|_| format!("invalid hour '{hour_s}' in time '{s}'"))?;
    let minute: u32 = minute_s
        .parse()
        .map_err(|_| format!("invalid minute '{minute_s}' in time '{s}'"))?;

    if let Some(is_pm) = ampm {
        if !(1..=12).contains(&hour) {
            return Err(format!("hour out of range for 12-hour time: '{s}'"));
        }
        if is_pm && hour != 12 {
            hour += 12;
        }
        if !is_pm && hour == 12 {
            hour = 0;
        }
    } else if hour > 23 || minute > 59 {
        return Err(format!("time out of range: '{s}'"));
    }

    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(|| format!("invalid time '{s}'"))
}

fn local_today(clock: &dyn Clock) -> NaiveDate {
    clock.now().with_timezone(&Local).date_naive()
}

/// Combine a local date + time into a UTC instant, disambiguating DST.
fn local_combine(date: NaiveDate, t: NaiveTime) -> Result<DateTime<Utc>, String> {
    let naive = date.and_time(t);
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(d) => Ok(d.with_timezone(&Utc)),
        // Spring-forward gap → bump to the next valid minute.
        chrono::LocalResult::None => {
            let bumped = naive + Duration::hours(1);
            Local
                .from_local_datetime(&bumped)
                .single()
                .map(|d| d.with_timezone(&Utc))
                .ok_or_else(|| format!("local datetime {naive} unresolvable"))
        }
        // Fall-back overlap → pick the earlier of the two.
        chrono::LocalResult::Ambiguous(a, _) => Ok(a.with_timezone(&Utc)),
    }
}

/// Compute the next time the cron expression fires, strictly after `now`.
fn next_cron_fire(cron: &str, clock: &dyn Clock) -> Result<DateTime<Utc>, String> {
    let schedule = CronSchedule::from_str(cron)
        .map_err(|e| format!("invalid cron expression '{cron}': {e}"))?;
    schedule
        .after(&clock.now())
        .next()
        .ok_or_else(|| format!("cron '{cron}' yields no future fire time"))
}

// ──────────────────────────────────────────────────────────────────────────
// Scheduler state + persistence
// ──────────────────────────────────────────────────────────────────────────

/// Heap entry — sorted ascending by `next_fire_at` using `Reverse` inside
/// the heap (see push/pop below).
#[derive(Debug, Clone)]
struct HeapItem {
    fire_at: DateTime<Utc>,
    entry_id: String,
}

// Ascending-order BinaryHeap via reverse Ord.
impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.fire_at == other.fire_at && self.entry_id == other.entry_id
    }
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Earlier fire_at = higher priority = "greater" in the max-heap.
        other.fire_at.cmp(&self.fire_at).then_with(|| other.entry_id.cmp(&self.entry_id))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub struct Scheduler {
    entries: Vec<ScheduleEntry>,
    heap: BinaryHeap<HeapItem>,
    path: PathBuf,
    clock: Arc<dyn Clock>,
}

impl Scheduler {
    /// Construct an empty scheduler that persists to `path`. Applies no
    /// catch-up (use [`Self::load`] for that).
    pub fn new(path: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self {
            entries: Vec::new(),
            heap: BinaryHeap::new(),
            path,
            clock,
        }
    }

    /// Load entries from `path`, apply per-entry catch-up policy, return a
    /// vector of immediate firings the consumer should dispatch before the
    /// next poll. Creates an empty jsonl if the file doesn't exist.
    pub fn load(path: PathBuf, clock: Arc<dyn Clock>) -> Result<(Self, Vec<Firing>), String> {
        let mut scheduler = Self::new(path.clone(), clock.clone());
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("scheduler: read {}: {e}", path.display())),
        };

        let now = clock.now();
        let mut immediate: Vec<Firing> = Vec::new();
        let mut parse_errors: usize = 0;

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut entry: ScheduleEntry = match serde_json::from_str(trimmed) {
                Ok(e) => e,
                Err(_) => {
                    parse_errors += 1;
                    continue;
                }
            };

            if entry.next_fire_at <= now {
                // Missed at least one firing while we were down.
                match (entry.catch_up, entry.kind.clone()) {
                    (CatchUp::Skip, ScheduleKind::OneShot) => continue, // drop
                    (CatchUp::Skip, ScheduleKind::Recurring) => {
                        if let Some(cron) = entry.recurrence.clone() {
                            match next_cron_fire(&cron, &*clock) {
                                Ok(next) => entry.next_fire_at = next,
                                Err(_) => continue,
                            }
                        }
                    }
                    (CatchUp::Once, ScheduleKind::OneShot) => {
                        immediate.push(Firing {
                            entry_id: entry.id.clone(),
                            prompt: entry.prompt.clone(),
                            chat_id: entry.chat_id,
                            scheduled_for: entry.next_fire_at,
                        });
                        continue; // one-shot fires and is removed
                    }
                    (CatchUp::Once, ScheduleKind::Recurring) => {
                        immediate.push(Firing {
                            entry_id: entry.id.clone(),
                            prompt: entry.prompt.clone(),
                            chat_id: entry.chat_id,
                            scheduled_for: entry.next_fire_at,
                        });
                        if let Some(cron) = entry.recurrence.clone() {
                            match next_cron_fire(&cron, &*clock) {
                                Ok(next) => entry.next_fire_at = next,
                                Err(_) => continue,
                            }
                        }
                    }
                    (CatchUp::All, ScheduleKind::OneShot) => {
                        immediate.push(Firing {
                            entry_id: entry.id.clone(),
                            prompt: entry.prompt.clone(),
                            chat_id: entry.chat_id,
                            scheduled_for: entry.next_fire_at,
                        });
                        continue;
                    }
                    (CatchUp::All, ScheduleKind::Recurring) => {
                        if let Some(cron) = entry.recurrence.clone() {
                            let Ok(schedule) = CronSchedule::from_str(&cron) else {
                                continue;
                            };
                            let mut fired_for = entry.next_fire_at;
                            let mut count = 0;
                            immediate.push(Firing {
                                entry_id: entry.id.clone(),
                                prompt: entry.prompt.clone(),
                                chat_id: entry.chat_id,
                                scheduled_for: fired_for,
                            });
                            count += 1;
                            for upcoming in schedule.after(&entry.next_fire_at) {
                                if upcoming > now || count >= MAX_CATCH_UP_ALL {
                                    entry.next_fire_at = upcoming;
                                    break;
                                }
                                immediate.push(Firing {
                                    entry_id: entry.id.clone(),
                                    prompt: entry.prompt.clone(),
                                    chat_id: entry.chat_id,
                                    scheduled_for: upcoming,
                                });
                                fired_for = upcoming;
                                count += 1;
                            }
                            // If the loop exited because count >= MAX, we still
                            // need a future next_fire_at for the entry to keep
                            // going. Skip past `now` conservatively.
                            if entry.next_fire_at <= now {
                                if let Some(next) = schedule.after(&now).next() {
                                    entry.next_fire_at = next;
                                }
                            }
                            let _ = fired_for;
                        }
                    }
                }
            }

            scheduler.heap.push(HeapItem {
                fire_at: entry.next_fire_at,
                entry_id: entry.id.clone(),
            });
            scheduler.entries.push(entry);
        }

        if parse_errors > 0 {
            eprintln!(
                "scheduler: ignored {parse_errors} malformed line(s) in {}",
                scheduler.path.display()
            );
        }

        // Persist post-catch-up state so the heap and file agree.
        scheduler.save()?;
        Ok((scheduler, immediate))
    }

    /// Add a new one-shot or recurring entry. Caller supplies the prompt,
    /// expression, and optional chat_id / catch_up override.
    pub fn add(
        &mut self,
        expr: &str,
        prompt: String,
        chat_id: Option<i64>,
        catch_up: Option<CatchUp>,
    ) -> Result<ScheduleEntry, String> {
        let parsed = parse_expression(expr, &*self.clock)?;
        let now = self.clock.now();
        let (kind, next_fire_at, recurrence) = match parsed {
            ParsedSchedule::OneShot { at } => (ScheduleKind::OneShot, at, None),
            ParsedSchedule::Recurring {
                cron,
                next_fire_at,
            } => (ScheduleKind::Recurring, next_fire_at, Some(cron)),
        };

        // Sensible default for catch-up: once for reminders, skip for
        // recurring briefings. Caller override wins.
        let catch_up = catch_up.unwrap_or(match kind {
            ScheduleKind::OneShot => CatchUp::Once,
            ScheduleKind::Recurring => CatchUp::Skip,
        });

        let entry = ScheduleEntry {
            id: new_id(now),
            kind,
            original_expr: expr.to_string(),
            next_fire_at,
            recurrence,
            prompt,
            chat_id,
            catch_up,
            created_at: now,
        };

        self.heap.push(HeapItem {
            fire_at: entry.next_fire_at,
            entry_id: entry.id.clone(),
        });
        self.entries.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    /// Cancel an entry by id. Returns `true` if the entry existed.
    pub fn cancel(&mut self, id: &str) -> Result<bool, String> {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        let removed = self.entries.len() < before;
        if removed {
            // Rebuild the heap — the lazy cancel via dead entries is fine
            // performance-wise (we expect at most ~100 entries), and this
            // keeps the next_due_at invariant obvious.
            self.heap = self
                .entries
                .iter()
                .map(|e| HeapItem {
                    fire_at: e.next_fire_at,
                    entry_id: e.id.clone(),
                })
                .collect();
            self.save()?;
        }
        Ok(removed)
    }

    /// Snapshot of active entries, in insertion order.
    #[must_use]
    pub fn list(&self) -> &[ScheduleEntry] {
        &self.entries
    }

    /// Earliest `next_fire_at` across all active entries, or `None` if
    /// empty. The consumer uses this to set its wake-up timer.
    #[must_use]
    pub fn next_due_at(&self) -> Option<DateTime<Utc>> {
        // Peek at the heap. Skip items whose entry_id has been cancelled
        // but not yet popped (shouldn't happen because `cancel` rebuilds,
        // but be defensive).
        for item in &self.heap {
            if self.entries.iter().any(|e| e.id == item.entry_id) {
                return Some(item.fire_at);
            }
        }
        None
    }

    /// Collect every entry whose `next_fire_at <= clock.now()`. Recurring
    /// entries advance to their next occurrence; one-shots are removed.
    /// Persists the new state to disk.
    pub fn fire_due(&mut self) -> Result<Vec<Firing>, String> {
        let now = self.clock.now();
        let mut firings: Vec<Firing> = Vec::new();

        // Find due entries by scanning (cheap at our scale).
        let due_ids: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.next_fire_at <= now)
            .map(|e| e.id.clone())
            .collect();

        if due_ids.is_empty() {
            return Ok(firings);
        }

        for id in &due_ids {
            // Collect the firing + decide the follow-up state.
            let Some(idx) = self.entries.iter().position(|e| &e.id == id) else {
                continue;
            };
            let fired_for = self.entries[idx].next_fire_at;
            firings.push(Firing {
                entry_id: id.clone(),
                prompt: self.entries[idx].prompt.clone(),
                chat_id: self.entries[idx].chat_id,
                scheduled_for: fired_for,
            });

            match self.entries[idx].kind.clone() {
                ScheduleKind::OneShot => {
                    self.entries.remove(idx);
                }
                ScheduleKind::Recurring => {
                    if let Some(cron) = self.entries[idx].recurrence.clone() {
                        match next_cron_fire(&cron, &*self.clock) {
                            Ok(next) => self.entries[idx].next_fire_at = next,
                            Err(_) => {
                                // Malformed recurrence — drop the entry so we
                                // don't hot-loop over it.
                                self.entries.remove(idx);
                            }
                        }
                    } else {
                        self.entries.remove(idx);
                    }
                }
            }
        }

        // Rebuild the heap from the post-fire state.
        self.heap = self
            .entries
            .iter()
            .map(|e| HeapItem {
                fire_at: e.next_fire_at,
                entry_id: e.id.clone(),
            })
            .collect();

        self.save()?;
        Ok(firings)
    }

    /// Write the current entries to jsonl atomically via write-and-rename.
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("scheduler: create {}: {e}", parent.display()))?;
        }
        let mut body = String::new();
        for entry in &self.entries {
            let line = serde_json::to_string(entry)
                .map_err(|e| format!("scheduler: serialize entry {}: {e}", entry.id))?;
            body.push_str(&line);
            body.push('\n');
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, &body)
            .map_err(|e| format!("scheduler: write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("scheduler: rename {} -> {}: {e}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Global singleton — shared between tool handlers (who add/cancel/list) and
// the telegram consumer (who fires due entries).
//
// The Telegram consumer should call [`install`] at startup with a freshly
// loaded scheduler so the immediate catch-up firings get dispatched. In
// REPL / single-shot / TUI modes there's no consumer, so [`global`] lazily
// inits with an empty scheduler that still persists to jsonl — any entries
// added there will be seen by the Telegram consumer next time it starts.
// ──────────────────────────────────────────────────────────────────────────

static GLOBAL: OnceLock<Mutex<Scheduler>> = OnceLock::new();

/// Default path for the persistent schedule file: `~/.claudette/schedule.jsonl`.
pub fn default_path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("schedule.jsonl")
}

/// Install a freshly-loaded scheduler as the process-wide singleton. Call
/// once from the Telegram consumer at startup; later calls are silently
/// ignored (the global is set-once).
pub fn install(scheduler: Scheduler) {
    let _ = GLOBAL.set(Mutex::new(scheduler));
}

/// Return the process-wide scheduler, lazy-initialising an empty one
/// against `default_path()` + `SystemClock` on first access.
pub fn global() -> &'static Mutex<Scheduler> {
    GLOBAL.get_or_init(|| {
        let path = default_path();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let scheduler = match Scheduler::load(path.clone(), clock.clone()) {
            // Fall-back path: any catch-up firings are discarded because
            // there's no consumer here to drain them. The entries still
            // persist to disk, so when the Telegram consumer next starts
            // and calls `install` with its own `load` result, those same
            // firings will be picked up then.
            Ok((s, _firings)) => s,
            Err(_) => Scheduler::new(path, clock),
        };
        Mutex::new(scheduler)
    })
}

/// Generate a new entry id `sch_<base36 nanos>` — collision-free for any
/// sane per-user call rate.
fn new_id(now: DateTime<Utc>) -> String {
    let nanos = now.timestamp_nanos_opt().unwrap_or(0).max(0) as u64;
    format!("sch_{}", base36(nanos))
}

fn base36(mut n: u64) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::with_capacity(13);
    while n > 0 {
        out.push(ALPHABET[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;

    fn fixed_clock(y: i32, m: u32, d: u32, h: u32, min: u32) -> Arc<MockClock> {
        Arc::new(MockClock::new(
            Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap(),
        ))
    }

    #[test]
    fn parse_in_minutes() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("in 30 minutes", &*c).unwrap();
        match p {
            ParsedSchedule::OneShot { at } => {
                assert_eq!(at, c.now() + Duration::minutes(30));
            }
            ParsedSchedule::Recurring { .. } => panic!("expected OneShot"),
        }
    }

    #[test]
    fn parse_in_hours_singular() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("in 1 hour", &*c).unwrap();
        match p {
            ParsedSchedule::OneShot { at } => {
                assert_eq!(at, c.now() + Duration::hours(1));
            }
            ParsedSchedule::Recurring { .. } => panic!("expected OneShot"),
        }
    }

    #[test]
    fn parse_in_days() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("in 2 days", &*c).unwrap();
        match p {
            ParsedSchedule::OneShot { at } => {
                assert_eq!(at, c.now() + Duration::days(2));
            }
            ParsedSchedule::Recurring { .. } => panic!("expected OneShot"),
        }
    }

    #[test]
    fn parse_in_rejects_zero_or_negative() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        assert!(parse_expression("in 0 minutes", &*c).is_err());
        assert!(parse_expression("in -5 minutes", &*c).is_err());
    }

    #[test]
    fn parse_in_rejects_unknown_unit() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let err = parse_expression("in 5 fortnights", &*c).unwrap_err();
        assert!(err.contains("unknown time unit"), "got: {err}");
    }

    #[test]
    fn parse_tomorrow_at_24h() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let _ = parse_expression("tomorrow at 15:00", &*c).unwrap();
    }

    #[test]
    fn parse_tomorrow_at_12h_pm() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let _ = parse_expression("tomorrow at 3pm", &*c).unwrap();
    }

    #[test]
    fn parse_rfc3339_passthrough() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("2026-05-01T09:00:00Z", &*c).unwrap();
        match p {
            ParsedSchedule::OneShot { at } => {
                assert_eq!(at, Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap());
            }
            ParsedSchedule::Recurring { .. } => panic!("expected OneShot"),
        }
    }

    #[test]
    fn parse_every_weekday_builds_cron() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("every weekday at 07:00", &*c).unwrap();
        match p {
            ParsedSchedule::Recurring { cron, .. } => {
                assert!(cron.contains("Mon-Fri"), "got: {cron}");
                assert!(cron.contains('7'), "got: {cron}");
            }
            ParsedSchedule::OneShot { .. } => panic!("expected Recurring"),
        }
    }

    #[test]
    fn parse_daily_builds_star_cron() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("daily at 08:30", &*c).unwrap();
        match p {
            ParsedSchedule::Recurring { cron, .. } => {
                // sec min hour dom mon dow year
                assert!(cron.starts_with("0 30 8 "), "got: {cron}");
            }
            ParsedSchedule::OneShot { .. } => panic!("expected Recurring"),
        }
    }

    #[test]
    fn parse_every_n_minutes() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("every 15 minutes", &*c).unwrap();
        match p {
            ParsedSchedule::Recurring { cron, .. } => {
                assert!(cron.contains("*/15"), "got: {cron}");
            }
            ParsedSchedule::OneShot { .. } => panic!("expected Recurring"),
        }
    }

    #[test]
    fn parse_cron_passthrough() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let p = parse_expression("cron: 0 0 7 * * Mon-Fri *", &*c).unwrap();
        match p {
            ParsedSchedule::Recurring { cron, .. } => {
                assert_eq!(cron, "0 0 7 * * mon-fri *");
            }
            ParsedSchedule::OneShot { .. } => panic!("expected Recurring"),
        }
    }

    #[test]
    fn parse_empty_errors() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        assert!(parse_expression("", &*c).is_err());
        assert!(parse_expression("   ", &*c).is_err());
    }

    #[test]
    fn parse_gibberish_errors_with_hint() {
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let err = parse_expression("sometime soon", &*c).unwrap_err();
        assert!(err.contains("could not parse"), "got: {err}");
        assert!(err.contains("Try"), "error should guide the model: {err}");
    }

    // Scheduler state tests ────────────────────────────────────────────

    fn tmp_path(label: &str) -> PathBuf {
        let base = std::env::temp_dir();
        base.join(format!(
            "claudette-scheduler-test-{label}-{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn add_oneshot_persists_and_next_due_reflects_it() {
        let path = tmp_path("oneshot");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let mut s = Scheduler::new(path.clone(), c.clone());
        let entry = s
            .add("in 30 minutes", "say hi".into(), Some(42), None)
            .unwrap();
        assert_eq!(entry.kind, ScheduleKind::OneShot);
        assert_eq!(s.list().len(), 1);
        assert_eq!(s.next_due_at(), Some(entry.next_fire_at));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fire_due_returns_ripe_oneshot_and_removes_it() {
        let path = tmp_path("ripe");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let mut s = Scheduler::new(path.clone(), c.clone());
        s.add("in 30 minutes", "p1".into(), None, None).unwrap();

        // Not due yet.
        let firings = s.fire_due().unwrap();
        assert!(firings.is_empty());
        assert_eq!(s.list().len(), 1);

        // Advance past fire time.
        c.advance(Duration::minutes(31));
        let firings = s.fire_due().unwrap();
        assert_eq!(firings.len(), 1);
        assert_eq!(firings[0].prompt, "p1");
        // One-shot removed.
        assert_eq!(s.list().len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fire_due_advances_recurring() {
        let path = tmp_path("recurring");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let mut s = Scheduler::new(path.clone(), c.clone());
        s.add("every 15 minutes", "poll".into(), None, None).unwrap();

        let first_fire_at = s.list()[0].next_fire_at;

        // Jump past first fire.
        c.set(first_fire_at + Duration::seconds(5));
        let firings = s.fire_due().unwrap();
        assert_eq!(firings.len(), 1);
        assert_eq!(s.list().len(), 1, "recurring should survive firing");
        let new_fire = s.list()[0].next_fire_at;
        assert!(
            new_fire > first_fire_at,
            "next_fire_at should advance (was {first_fire_at}, now {new_fire})"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_removes_entry() {
        let path = tmp_path("cancel");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let mut s = Scheduler::new(path.clone(), c.clone());
        let e = s.add("in 1 hour", "x".into(), None, None).unwrap();
        assert_eq!(s.list().len(), 1);

        let removed = s.cancel(&e.id).unwrap();
        assert!(removed);
        assert_eq!(s.list().len(), 0);
        assert!(s.next_due_at().is_none());

        // Cancel of unknown id returns false, doesn't error.
        assert!(!s.cancel("sch_nosuch").unwrap());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_file_returns_empty_scheduler() {
        let path = tmp_path("missing");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let (s, immediate) = Scheduler::load(path.clone(), c.clone()).unwrap();
        assert!(immediate.is_empty());
        assert_eq!(s.list().len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn catch_up_once_returns_missed_oneshot() {
        let path = tmp_path("catchup-once");
        let boot = fixed_clock(2026, 4, 21, 10, 0);
        {
            // Write jsonl as if an entry fired 5 min ago.
            let past = boot.now() - Duration::minutes(5);
            let entry = ScheduleEntry {
                id: "sch_past1".into(),
                kind: ScheduleKind::OneShot,
                original_expr: "in 5 minutes".into(),
                next_fire_at: past,
                recurrence: None,
                prompt: "missed reminder".into(),
                chat_id: Some(1),
                catch_up: CatchUp::Once,
                created_at: past - Duration::minutes(10),
            };
            let line = serde_json::to_string(&entry).unwrap();
            let _ = std::fs::write(&path, format!("{line}\n"));
        }

        let (s, immediate) = Scheduler::load(path.clone(), boot.clone()).unwrap();
        assert_eq!(immediate.len(), 1);
        assert_eq!(immediate[0].prompt, "missed reminder");
        assert_eq!(s.list().len(), 0, "one-shot should be gone post-fire");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn catch_up_skip_drops_missed_recurring() {
        let path = tmp_path("catchup-skip");
        let boot = fixed_clock(2026, 4, 21, 12, 0);
        {
            let past = boot.now() - Duration::hours(5);
            let entry = ScheduleEntry {
                id: "sch_skip1".into(),
                kind: ScheduleKind::Recurring,
                original_expr: "daily at 07:00".into(),
                next_fire_at: past,
                recurrence: Some("0 0 7 * * * *".into()),
                prompt: "briefing".into(),
                chat_id: Some(1),
                catch_up: CatchUp::Skip,
                created_at: past - Duration::days(1),
            };
            let line = serde_json::to_string(&entry).unwrap();
            let _ = std::fs::write(&path, format!("{line}\n"));
        }

        let (s, immediate) = Scheduler::load(path.clone(), boot.clone()).unwrap();
        assert_eq!(immediate.len(), 0, "skip should fire nothing");
        assert_eq!(s.list().len(), 1, "recurring survives");
        assert!(
            s.list()[0].next_fire_at > boot.now(),
            "next_fire_at must be in the future"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_default_catch_up_is_once_for_oneshot_and_skip_for_recurring() {
        let path = tmp_path("defaults");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        let mut s = Scheduler::new(path.clone(), c.clone());

        let a = s.add("in 30 minutes", "a".into(), None, None).unwrap();
        assert_eq!(a.catch_up, CatchUp::Once);

        let b = s.add("daily at 07:00", "b".into(), None, None).unwrap();
        assert_eq!(b.catch_up, CatchUp::Skip);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_and_reload_roundtrip_preserves_entries() {
        let path = tmp_path("roundtrip");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        {
            let mut s = Scheduler::new(path.clone(), c.clone());
            s.add("in 2 hours", "first".into(), Some(99), None).unwrap();
            s.add("daily at 08:00", "second".into(), Some(99), None)
                .unwrap();
        }

        let (s2, _) = Scheduler::load(path.clone(), c.clone()).unwrap();
        assert_eq!(s2.list().len(), 2);
        let prompts: Vec<_> = s2.list().iter().map(|e| e.prompt.clone()).collect();
        assert!(prompts.contains(&"first".to_string()));
        assert!(prompts.contains(&"second".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_line_is_ignored_not_fatal() {
        let path = tmp_path("malformed");
        let c = fixed_clock(2026, 4, 21, 10, 0);
        {
            let _ = std::fs::write(&path, "not-json garbage\n");
        }
        let (s, _) = Scheduler::load(path.clone(), c.clone()).unwrap();
        assert!(s.list().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn new_id_shape_is_stable() {
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 10, 0, 0).unwrap();
        let id = new_id(now);
        assert!(id.starts_with("sch_"), "got: {id}");
        assert!(id.len() > 5);
    }
}
