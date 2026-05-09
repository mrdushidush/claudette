//! Google Calendar group — 5 tools against the Calendar v3 REST API.
//!
//! Authentication: every call fetches a fresh bearer via
//! [`crate::google_auth::access_token`], which transparently refreshes when
//! the stored token is close to expiry. The user must have run
//! `claudette --auth-google` at least once; otherwise the tool returns a
//! descriptive error.
//!
//! Self-contained: helpers (`calendar_get`, `calendar_post`, `calendar_patch`,
//! `calendar_delete`, `default_calendar_id`) are private to this module.

use std::fmt::Write as _;

use chrono::{Duration, Utc};
use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "calendar_list_events",
                "description": "List Google Calendar events in a time range. Defaults to the next 7 days on the user's primary calendar. Requires `claudette --auth-google` one-time setup.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "time_min": { "type": "string", "description": "RFC3339 lower bound (inclusive). Default: now." },
                        "time_max": { "type": "string", "description": "RFC3339 upper bound (exclusive). Default: 7 days from now." },
                        "calendar_id": { "type": "string", "description": "Calendar ID or email. Default: 'primary'." },
                        "max_results": { "type": "number", "description": "Max events to return. Default: 25." }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "calendar_create_event",
                "description": "Create a Google Calendar event on the user's primary calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "summary":     { "type": "string", "description": "Event title." },
                        "start":       { "type": "string", "description": "RFC3339 start datetime (e.g. 2026-04-22T15:00:00-04:00)." },
                        "end":         { "type": "string", "description": "RFC3339 end datetime." },
                        "description": { "type": "string", "description": "Event description (optional)." },
                        "location":    { "type": "string", "description": "Event location (optional)." },
                        "attendees":   { "type": "array", "description": "Attendee emails (optional).", "items": { "type": "string" } },
                        "calendar_id": { "type": "string", "description": "Calendar ID. Default: 'primary'." }
                    },
                    "required": ["summary", "start", "end"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "calendar_update_event",
                "description": "Patch fields on an existing Google Calendar event. Only supplied fields are changed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_id":    { "type": "string", "description": "The event ID returned by create/list." },
                        "summary":     { "type": "string" },
                        "start":       { "type": "string", "description": "RFC3339 start datetime." },
                        "end":         { "type": "string", "description": "RFC3339 end datetime." },
                        "description": { "type": "string" },
                        "location":    { "type": "string" },
                        "calendar_id": { "type": "string", "description": "Calendar ID. Default: 'primary'." }
                    },
                    "required": ["event_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "calendar_delete_event",
                "description": "Delete a Google Calendar event. Irreversible — confirm with the user first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_id":    { "type": "string", "description": "The event ID to delete." },
                        "calendar_id": { "type": "string", "description": "Calendar ID. Default: 'primary'." }
                    },
                    "required": ["event_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "calendar_respond_to_event",
                "description": "RSVP to a Google Calendar event as the current user (accepted, declined, or tentative).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_id":    { "type": "string", "description": "The event ID to respond to." },
                        "response":    { "type": "string", "enum": ["accepted", "declined", "tentative"], "description": "RSVP value." },
                        "calendar_id": { "type": "string", "description": "Calendar ID. Default: 'primary'." }
                    },
                    "required": ["event_id", "response"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "calendar_list_events" => run_list_events(input),
        "calendar_create_event" => run_create_event(input),
        "calendar_update_event" => run_update_event(input),
        "calendar_delete_event" => run_delete_event(input),
        "calendar_respond_to_event" => run_respond_to_event(input),
        _ => return None,
    };
    Some(result)
}

/// Percent-encode a path segment (calendar ID can contain `@` and `.`).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn default_calendar_id(v: &Value) -> String {
    v.get("calendar_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("primary")
        .to_string()
}

fn auth_header(
    builder: reqwest::blocking::RequestBuilder,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    builder
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
}

fn run_list_events(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "calendar_list_events")?;
    let calendar_id = default_calendar_id(&v);

    let time_min = v
        .get("time_min")
        .and_then(Value::as_str)
        .map_or_else(|| Utc::now().to_rfc3339(), str::to_string);
    let time_max = v.get("time_max").and_then(Value::as_str).map_or_else(
        || (Utc::now() + Duration::days(7)).to_rfc3339(),
        str::to_string,
    );
    let max_results = v
        .get("max_results")
        .and_then(Value::as_i64)
        .filter(|n| *n > 0 && *n <= 250)
        .unwrap_or(25)
        .to_string();

    let token = crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar)?;
    let client = external_http_client()?;
    let url = format!(
        "{API_BASE}/calendars/{cal}/events",
        cal = encode_segment(&calendar_id)
    );
    let resp = auth_header(client.get(&url), &token)
        .query(&[
            ("timeMin", time_min.as_str()),
            ("timeMax", time_max.as_str()),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            ("maxResults", max_results.as_str()),
        ])
        .send()
        .map_err(|e| format!("calendar_list_events: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_list_events: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("calendar_list_events: parse failed: {e}"))?;

    let items: Vec<Value> = data
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(summarize_event).collect())
        .unwrap_or_default();

    Ok(json!({
        "calendar_id": calendar_id,
        "time_min": time_min,
        "time_max": time_max,
        "count": items.len(),
        "events": items,
    })
    .to_string())
}

/// Shrink a Calendar event JSON to the fields the model typically needs
/// without blowing out the context window. Full `data` is ~2 KB per event;
/// this keeps it to ~300 B.
fn summarize_event(e: &Value) -> Value {
    let attendees: Vec<Value> = e
        .get("attendees")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    json!({
                        "email": a.get("email").and_then(Value::as_str).unwrap_or(""),
                        "response_status": a.get("responseStatus").and_then(Value::as_str).unwrap_or(""),
                        "self": a.get("self").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "id": e.get("id").and_then(Value::as_str).unwrap_or(""),
        "summary": e.get("summary").and_then(Value::as_str).unwrap_or(""),
        "description": e.get("description").and_then(Value::as_str).unwrap_or("").chars().take(500).collect::<String>(),
        "location": e.get("location").and_then(Value::as_str).unwrap_or(""),
        "start": e.pointer("/start/dateTime").and_then(Value::as_str)
            .or_else(|| e.pointer("/start/date").and_then(Value::as_str))
            .unwrap_or(""),
        "end": e.pointer("/end/dateTime").and_then(Value::as_str)
            .or_else(|| e.pointer("/end/date").and_then(Value::as_str))
            .unwrap_or(""),
        "all_day": e.pointer("/start/date").is_some(),
        "status": e.get("status").and_then(Value::as_str).unwrap_or(""),
        "html_link": e.get("htmlLink").and_then(Value::as_str).unwrap_or(""),
        "attendees": attendees,
    })
}

fn run_create_event(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "calendar_create_event")?;
    let summary = extract_str(&v, "summary", "calendar_create_event")?;
    let start = extract_str(&v, "start", "calendar_create_event")?;
    let end = extract_str(&v, "end", "calendar_create_event")?;
    let calendar_id = default_calendar_id(&v);

    let mut payload = json!({
        "summary": summary,
        "start": { "dateTime": start },
        "end":   { "dateTime": end },
    });
    if let Some(description) = v.get("description").and_then(Value::as_str) {
        payload["description"] = Value::String(description.to_string());
    }
    if let Some(location) = v.get("location").and_then(Value::as_str) {
        payload["location"] = Value::String(location.to_string());
    }
    if let Some(attendees) = v.get("attendees").and_then(Value::as_array) {
        let arr: Vec<Value> = attendees
            .iter()
            .filter_map(|a| a.as_str())
            .map(|email| json!({ "email": email }))
            .collect();
        if !arr.is_empty() {
            payload["attendees"] = Value::Array(arr);
        }
    }

    let token = crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar)?;
    let client = external_http_client()?;
    let url = format!(
        "{API_BASE}/calendars/{cal}/events",
        cal = encode_segment(&calendar_id)
    );
    let resp = auth_header(client.post(&url), &token)
        .json(&payload)
        .send()
        .map_err(|e| format!("calendar_create_event: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_create_event: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("calendar_create_event: parse failed: {e}"))?;
    Ok(json!({
        "ok": true,
        "event": summarize_event(&data),
    })
    .to_string())
}

fn run_update_event(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "calendar_update_event")?;
    let event_id = extract_str(&v, "event_id", "calendar_update_event")?;
    let calendar_id = default_calendar_id(&v);

    let mut payload = serde_json::Map::new();
    if let Some(x) = v.get("summary").and_then(Value::as_str) {
        payload.insert("summary".into(), Value::String(x.to_string()));
    }
    if let Some(x) = v.get("description").and_then(Value::as_str) {
        payload.insert("description".into(), Value::String(x.to_string()));
    }
    if let Some(x) = v.get("location").and_then(Value::as_str) {
        payload.insert("location".into(), Value::String(x.to_string()));
    }
    if let Some(x) = v.get("start").and_then(Value::as_str) {
        payload.insert("start".into(), json!({ "dateTime": x }));
    }
    if let Some(x) = v.get("end").and_then(Value::as_str) {
        payload.insert("end".into(), json!({ "dateTime": x }));
    }
    if payload.is_empty() {
        return Err(
            "calendar_update_event: no fields to update (pass at least one of summary, start, end, description, location)"
                .to_string(),
        );
    }

    let token = crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar)?;
    let client = external_http_client()?;
    let url = format!(
        "{API_BASE}/calendars/{cal}/events/{eid}",
        cal = encode_segment(&calendar_id),
        eid = encode_segment(event_id),
    );
    let resp = auth_header(client.patch(&url), &token)
        .json(&Value::Object(payload))
        .send()
        .map_err(|e| format!("calendar_update_event: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_update_event: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("calendar_update_event: parse failed: {e}"))?;
    Ok(json!({
        "ok": true,
        "event": summarize_event(&data),
    })
    .to_string())
}

fn run_delete_event(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "calendar_delete_event")?;
    let event_id = extract_str(&v, "event_id", "calendar_delete_event")?;
    let calendar_id = default_calendar_id(&v);

    let token = crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar)?;
    let client = external_http_client()?;
    let url = format!(
        "{API_BASE}/calendars/{cal}/events/{eid}",
        cal = encode_segment(&calendar_id),
        eid = encode_segment(event_id),
    );
    let resp = auth_header(client.delete(&url), &token)
        .send()
        .map_err(|e| format!("calendar_delete_event: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        return Err(format!(
            "calendar_delete_event: event '{event_id}' not found on calendar '{calendar_id}'"
        ));
    }
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_delete_event: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    Ok(json!({
        "ok": true,
        "deleted": true,
        "event_id": event_id,
        "calendar_id": calendar_id,
    })
    .to_string())
}

fn run_respond_to_event(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "calendar_respond_to_event")?;
    let event_id = extract_str(&v, "event_id", "calendar_respond_to_event")?;
    let response = extract_str(&v, "response", "calendar_respond_to_event")?;
    let calendar_id = default_calendar_id(&v);

    if !matches!(response, "accepted" | "declined" | "tentative") {
        return Err(format!(
            "calendar_respond_to_event: invalid response '{response}' \
             (must be one of: accepted, declined, tentative)"
        ));
    }

    let token = crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar)?;
    let client = external_http_client()?;

    // Two-step dance: GET event to find the attendee list, PATCH with our
    // responseStatus updated. We use the `self: true` attendee entry Google
    // marks for the authenticated user.
    let url = format!(
        "{API_BASE}/calendars/{cal}/events/{eid}",
        cal = encode_segment(&calendar_id),
        eid = encode_segment(event_id),
    );
    let resp = auth_header(client.get(&url), &token)
        .send()
        .map_err(|e| format!("calendar_respond_to_event: GET failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_respond_to_event: GET HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let event: Value = resp
        .json()
        .map_err(|e| format!("calendar_respond_to_event: GET parse failed: {e}"))?;

    let attendees = event
        .get("attendees")
        .and_then(Value::as_array)
        .ok_or("calendar_respond_to_event: event has no attendees list")?;

    let mut updated: Vec<Value> = attendees.clone();
    let mut mutated = false;
    for a in &mut updated {
        if a.get("self").and_then(Value::as_bool).unwrap_or(false) {
            a["responseStatus"] = Value::String(response.to_string());
            mutated = true;
        }
    }
    if !mutated {
        return Err(
            "calendar_respond_to_event: current user is not listed as an attendee on this event"
                .to_string(),
        );
    }

    let payload = json!({ "attendees": updated });
    let resp = auth_header(client.patch(&url), &token)
        .json(&payload)
        .send()
        .map_err(|e| format!("calendar_respond_to_event: PATCH failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "calendar_respond_to_event: PATCH HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let data: Value = resp
        .json()
        .map_err(|e| format!("calendar_respond_to_event: PATCH parse failed: {e}"))?;

    Ok(json!({
        "ok": true,
        "response": response,
        "event": summarize_event(&data),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_five_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "calendar_list_events",
                "calendar_create_event",
                "calendar_update_event",
                "calendar_delete_event",
                "calendar_respond_to_event",
            ]
        );
    }

    #[test]
    fn create_event_rejects_missing_summary() {
        let err =
            run_create_event(r#"{"start":"2026-01-01T00:00:00Z","end":"2026-01-01T01:00:00Z"}"#)
                .unwrap_err();
        assert!(err.contains("summary"), "got: {err}");
    }

    #[test]
    fn create_event_rejects_missing_end() {
        let err =
            run_create_event(r#"{"summary":"x","start":"2026-01-01T00:00:00Z"}"#).unwrap_err();
        assert!(err.contains("end"), "got: {err}");
    }

    #[test]
    fn update_event_rejects_missing_event_id() {
        let err = run_update_event(r#"{"summary":"x"}"#).unwrap_err();
        assert!(err.contains("event_id"), "got: {err}");
    }

    #[test]
    fn update_event_rejects_empty_patch() {
        // Cannot test without real token; but if access_token errors first
        // the message is about auth, not empty payload. Craft a case where
        // access_token doesn't get called by violating pre-auth validation:
        // pass event_id but no fields. The module checks payload BEFORE
        // calling access_token only when event_id passes, so we need to
        // intercept. Skip if user happens to be authenticated.
        if crate::google_auth::access_token(crate::google_auth::AuthContext::Calendar).is_ok() {
            return;
        }
        let err = run_update_event(r#"{"event_id":"abc"}"#).unwrap_err();
        // Either the empty-payload error or the auth error is acceptable;
        // both signal the handler rejected the call without a real request.
        assert!(
            err.contains("no fields") || err.contains("not authenticated"),
            "got: {err}"
        );
    }

    #[test]
    fn delete_event_rejects_missing_event_id() {
        let err = run_delete_event("{}").unwrap_err();
        assert!(err.contains("event_id"), "got: {err}");
    }

    #[test]
    fn respond_to_event_rejects_missing_event_id() {
        let err = run_respond_to_event(r#"{"response":"accepted"}"#).unwrap_err();
        assert!(err.contains("event_id"), "got: {err}");
    }

    #[test]
    fn respond_to_event_rejects_bad_response_value() {
        // Skip if the user is actually authenticated — the handler validates
        // the response value BEFORE calling access_token, so this case is
        // covered regardless.
        let err = run_respond_to_event(r#"{"event_id":"abc","response":"maybe"}"#).unwrap_err();
        assert!(err.contains("invalid response"), "got: {err}");
    }

    #[test]
    fn list_events_defaults_calendar_id_to_primary() {
        let v = serde_json::json!({});
        assert_eq!(default_calendar_id(&v), "primary");
        let v = serde_json::json!({ "calendar_id": "foo@example.com" });
        assert_eq!(default_calendar_id(&v), "foo@example.com");
        // Empty string falls back to primary.
        let v = serde_json::json!({ "calendar_id": "" });
        assert_eq!(default_calendar_id(&v), "primary");
    }

    #[test]
    fn encode_segment_escapes_at_and_colon() {
        assert_eq!(encode_segment("foo@example.com"), "foo%40example.com");
        assert_eq!(encode_segment("abc:def"), "abc%3Adef");
        assert_eq!(encode_segment("primary"), "primary");
    }

    #[test]
    fn summarize_event_extracts_common_fields() {
        let raw = serde_json::json!({
            "id": "evt123",
            "summary": "Team sync",
            "description": "Weekly catch-up",
            "location": "Zoom",
            "start": { "dateTime": "2026-04-22T15:00:00-04:00" },
            "end":   { "dateTime": "2026-04-22T15:30:00-04:00" },
            "status": "confirmed",
            "htmlLink": "https://calendar.google.com/event?id=evt123",
            "attendees": [
                { "email": "me@x.com", "responseStatus": "accepted", "self": true },
                { "email": "other@x.com", "responseStatus": "needsAction" }
            ]
        });
        let out = summarize_event(&raw);
        assert_eq!(out["id"], "evt123");
        assert_eq!(out["summary"], "Team sync");
        assert_eq!(out["start"], "2026-04-22T15:00:00-04:00");
        assert_eq!(out["all_day"], false);
        let attendees = out["attendees"].as_array().unwrap();
        assert_eq!(attendees.len(), 2);
        assert_eq!(attendees[0]["self"], true);
    }

    #[test]
    fn summarize_event_handles_all_day() {
        let raw = serde_json::json!({
            "id": "x",
            "summary": "Holiday",
            "start": { "date": "2026-07-04" },
            "end":   { "date": "2026-07-05" },
        });
        let out = summarize_event(&raw);
        assert_eq!(out["all_day"], true);
        assert_eq!(out["start"], "2026-07-04");
    }
}
