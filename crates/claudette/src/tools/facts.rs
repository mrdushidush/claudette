//! Facts group — Wikipedia lookups and Open-Meteo weather. Four tools,
//! all stateless HTTP, no API keys.
//!
//! Parent-module helpers used: `parse_json_input`, `extract_str`,
//! `external_http_client`, and `strip_html` (shared with the search group's
//! `web_fetch`). The geocoder / Hebrew-alias / WMO-label helpers are
//! facts-only and live private in this module.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input, strip_html};

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "wikipedia_search",
                "description": "Search Wikipedia for article titles matching a query. Returns top 5 hits.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search terms" }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "wikipedia_summary",
                "description": "Get a plain-text summary of a Wikipedia article by exact title.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Exact article title (use wikipedia_search first if unsure)" }
                    },
                    "required": ["title"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "weather_current",
                "description": "Current weather for a city or 'lat,lon'. No API key needed. Uses Open-Meteo.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name (e.g. 'Paris') or 'lat,lon'" }
                    },
                    "required": ["location"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "weather_forecast",
                "description": "Multi-day weather forecast for a city or 'lat,lon'. No API key needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name or 'lat,lon'" },
                        "days":     { "type": "number", "description": "Number of days (1-7, default 3)" }
                    },
                    "required": ["location"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "wikipedia_search" => run_wikipedia_search(input),
        "wikipedia_summary" => run_wikipedia_summary(input),
        "weather_current" => run_weather_current(input),
        "weather_forecast" => run_weather_forecast(input),
        _ => return None,
    };
    Some(result)
}

// ────── Wikipedia ────────────────────────────────────────────────────────

fn run_wikipedia_search(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "wikipedia_search")?;
    let query = extract_str(&v, "query", "wikipedia_search")?.to_string();

    let client = external_http_client()?;
    let resp = client
        .get("https://en.wikipedia.org/w/api.php")
        .query(&[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", query.as_str()),
            ("format", "json"),
            ("srlimit", "5"),
        ])
        .send()
        .map_err(|e| format!("wikipedia_search: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("wikipedia_search: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("wikipedia_search: parse failed: {e}"))?;

    let results: Vec<Value> = data
        .pointer("/query/search")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    let snippet = r
                        .get("snippet")
                        .and_then(Value::as_str)
                        .map(strip_html)
                        .unwrap_or_default();
                    json!({
                        "title": r.get("title").and_then(Value::as_str).unwrap_or(""),
                        "snippet": snippet,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_wikipedia_summary(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "wikipedia_summary")?;
    let title = extract_str(&v, "title", "wikipedia_summary")?;
    // Wikipedia REST API uses underscore-separated titles in the path.
    let encoded = title.replace(' ', "_");
    let url = format!("https://en.wikipedia.org/api/rest_v1/page/summary/{encoded}");

    let client = external_http_client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("wikipedia_summary: request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("wikipedia_summary: no article titled '{title}'"));
    }
    if !status.is_success() {
        return Err(format!("wikipedia_summary: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("wikipedia_summary: parse failed: {e}"))?;

    Ok(json!({
        "title": data.get("title").and_then(Value::as_str).unwrap_or(title),
        "extract": data.get("extract").and_then(Value::as_str).unwrap_or(""),
        "url": data
            .pointer("/content_urls/desktop/page")
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
    .to_string())
}

// ────── Open-Meteo weather ───────────────────────────────────────────────

/// Translate Hebrew (and common transliterated) city names to their English
/// equivalents for the Open-Meteo geocoding API. Covers the ~30 most
/// populated Israeli cities plus a few common variants.
fn hebrew_city_alias(name: &str) -> Option<&'static str> {
    // Normalize: trim, lowercase for Latin comparisons.
    let trimmed = name.trim();
    match trimmed {
        // Hebrew script
        "ירושלים" => Some("Jerusalem"),
        "תל אביב" | "תל-אביב" | "תל אביב יפו" | "תל-אביב-יפו" => {
            Some("Tel Aviv")
        }
        "חיפה" => Some("Haifa"),
        "ראשון לציון" | "ראשון-לציון" => Some("Rishon LeZion"),
        "פתח תקווה" | "פתח-תקווה" | "פתח תקוה" => Some("Petah Tikva"),
        "אשדוד" => Some("Ashdod"),
        "נתניה" => Some("Netanya"),
        "באר שבע" | "באר-שבע" | "בארשבע" => Some("Beer Sheva"),
        "חולון" => Some("Holon"),
        "בני ברק" | "בני-ברק" => Some("Bnei Brak"),
        "רמת גן" | "רמת-גן" => Some("Ramat Gan"),
        "אשקלון" => Some("Ashkelon"),
        "רחובות" => Some("Rehovot"),
        "בת ים" | "בת-ים" => Some("Bat Yam"),
        "הרצליה" => Some("Herzliya"),
        "כפר סבא" | "כפר-סבא" => Some("Kfar Saba"),
        "חדרה" => Some("Hadera"),
        "מודיעין" | "מודיעין-מכבים-רעות" => Some("Modiin"),
        "לוד" => Some("Lod"),
        "רמלה" => Some("Ramla"),
        "נצרת" => Some("Nazareth"),
        "עכו" => Some("Acre"),
        "אילת" => Some("Eilat"),
        "טבריה" => Some("Tiberias"),
        "צפת" => Some("Safed"),
        "עפולה" => Some("Afula"),
        "קריית גת" | "קריית-גת" => Some("Kiryat Gat"),
        "נהריה" => Some("Nahariya"),
        "גבעתיים" => Some("Givatayim"),
        "רעננה" => Some("Raanana"),
        _ => {
            // Also handle common Latin transliterations that the API misses.
            match trimmed.to_lowercase().as_str() {
                "hedera" | "khadera" => Some("Hadera"),
                "beer sheva" | "beersheva" | "be'er sheva" => Some("Beer Sheva"),
                "petach tikva" | "petach-tikva" | "petah-tikva" => Some("Petah Tikva"),
                "rishon lezion" | "rishon-lezion" => Some("Rishon LeZion"),
                "bnei brak" | "bnei-brak" => Some("Bnei Brak"),
                "ramat-gan" => Some("Ramat Gan"),
                "kfar saba" | "kfar-saba" => Some("Kfar Saba"),
                "bat-yam" => Some("Bat Yam"),
                _ => None,
            }
        }
    }
}

/// Geocode a free-text location into (lat, lon, display name) via Open-Meteo.
/// Accepts `"lat,lon"` shorthand for pre-resolved coordinates.
fn resolve_location(location: &str) -> Result<(f64, f64, String), String> {
    let trimmed = location.trim();
    // Shortcut: accept "lat,lon" directly.
    if let Some((lat_s, lon_s)) = trimmed.split_once(',') {
        if let (Ok(lat), Ok(lon)) = (lat_s.trim().parse::<f64>(), lon_s.trim().parse::<f64>()) {
            return Ok((lat, lon, format!("{lat:.4},{lon:.4}")));
        }
    }

    // Translate Hebrew / transliterated city names before geocoding.
    let lookup_name = hebrew_city_alias(trimmed).unwrap_or(trimmed);

    let client = external_http_client()?;
    let resp = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", lookup_name),
            ("count", "1"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()
        .map_err(|e| format!("geocoding: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("geocoding: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("geocoding: parse failed: {e}"))?;

    let first = data
        .pointer("/results/0")
        .ok_or_else(|| format!("geocoding: no match for '{location}'"))?;

    let lat = first
        .get("latitude")
        .and_then(Value::as_f64)
        .ok_or("geocoding: missing latitude")?;
    let lon = first
        .get("longitude")
        .and_then(Value::as_f64)
        .ok_or("geocoding: missing longitude")?;
    let name = first
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(trimmed)
        .to_string();
    let country = first
        .get("country")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let display = if country.is_empty() {
        name
    } else {
        format!("{name}, {country}")
    };
    Ok((lat, lon, display))
}

/// Convert a WMO weather code to a human label. Codes are documented at
/// <https://open-meteo.com/en/docs> — we only cover the common buckets so the
/// description stays short.
fn wmo_label(code: i64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 | 63 | 65 => "rain",
        66 | 67 => "freezing rain",
        71 | 73 | 75 => "snow",
        77 => "snow grains",
        80..=82 => "rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown",
    }
}

fn run_weather_current(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "weather_current")?;
    let location = extract_str(&v, "location", "weather_current")?;
    let (lat, lon, display) = resolve_location(location)?;

    let client = external_http_client()?;
    let resp = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string().as_str()),
            ("longitude", lon.to_string().as_str()),
            (
                "current",
                "temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m",
            ),
            ("timezone", "auto"),
            ("temperature_unit", "celsius"),
            ("wind_speed_unit", "kmh"),
        ])
        .send()
        .map_err(|e| format!("weather_current: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("weather_current: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("weather_current: parse failed: {e}"))?;

    let current = data
        .get("current")
        .ok_or("weather_current: response missing 'current'")?;
    let code = current
        .get("weather_code")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    let temp = current.get("temperature_2m").and_then(Value::as_f64);
    let feels = current.get("apparent_temperature").and_then(Value::as_f64);
    let humidity = current.get("relative_humidity_2m").and_then(Value::as_f64);
    let wind = current.get("wind_speed_10m").and_then(Value::as_f64);
    let time = current.get("time").and_then(Value::as_str).unwrap_or("");

    Ok(json!({
        "location": display,
        "latitude": lat,
        "longitude": lon,
        "time": time,
        "condition": wmo_label(code),
        "weather_code": code,
        "temperature_c": temp,
        "feels_like_c": feels,
        "humidity_pct": humidity,
        "wind_kmh": wind,
    })
    .to_string())
}

fn run_weather_forecast(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "weather_forecast")?;
    let location = extract_str(&v, "location", "weather_forecast")?;
    let days = v
        .get("days")
        .and_then(Value::as_i64)
        .unwrap_or(3)
        .clamp(1, 7);
    let (lat, lon, display) = resolve_location(location)?;

    let client = external_http_client()?;
    let resp = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string().as_str()),
            ("longitude", lon.to_string().as_str()),
            (
                "daily",
                "weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum",
            ),
            ("timezone", "auto"),
            ("temperature_unit", "celsius"),
            ("forecast_days", days.to_string().as_str()),
        ])
        .send()
        .map_err(|e| format!("weather_forecast: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("weather_forecast: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("weather_forecast: parse failed: {e}"))?;

    let daily = data
        .get("daily")
        .ok_or("weather_forecast: response missing 'daily'")?;
    let dates = daily
        .get("time")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let codes = daily
        .get("weather_code")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let maxes = daily
        .get("temperature_2m_max")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mins = daily
        .get("temperature_2m_min")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let precips = daily
        .get("precipitation_sum")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let days_out: Vec<Value> = (0..dates.len())
        .map(|i| {
            let code = codes.get(i).and_then(Value::as_i64).unwrap_or(-1);
            json!({
                "date": dates.get(i).and_then(Value::as_str).unwrap_or(""),
                "condition": wmo_label(code),
                "weather_code": code,
                "max_c": maxes.get(i).and_then(Value::as_f64),
                "min_c": mins.get(i).and_then(Value::as_f64),
                "precipitation_mm": precips.get(i).and_then(Value::as_f64),
            })
        })
        .collect();

    Ok(json!({
        "location": display,
        "latitude": lat,
        "longitude": lon,
        "days": days_out,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikipedia_search_rejects_missing_query() {
        let err = run_wikipedia_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn wikipedia_summary_rejects_missing_title() {
        let err = run_wikipedia_summary("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn weather_rejects_missing_location() {
        let err = run_weather_current("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
        let err = run_weather_forecast("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    // Offline unit tests for the facts-private helpers — no network.

    #[test]
    fn resolve_location_accepts_lat_lon_shortcut() {
        let (lat, lon, display) = resolve_location("48.8566, 2.3522").unwrap();
        assert!((lat - 48.8566).abs() < 1e-6);
        assert!((lon - 2.3522).abs() < 1e-6);
        assert_eq!(display, "48.8566,2.3522");
    }

    #[test]
    fn hebrew_city_alias_maps_hebrew_to_english() {
        assert_eq!(hebrew_city_alias("ירושלים"), Some("Jerusalem"));
        assert_eq!(hebrew_city_alias("תל אביב"), Some("Tel Aviv"));
        assert_eq!(hebrew_city_alias("חיפה"), Some("Haifa"));
    }

    #[test]
    fn hebrew_city_alias_maps_transliterations() {
        assert_eq!(hebrew_city_alias("Hedera"), Some("Hadera"));
        assert_eq!(hebrew_city_alias("beersheva"), Some("Beer Sheva"));
        assert_eq!(hebrew_city_alias("RISHON lezion"), Some("Rishon LeZion"));
    }

    #[test]
    fn hebrew_city_alias_returns_none_for_regular_name() {
        assert_eq!(hebrew_city_alias("Paris"), None);
        assert_eq!(hebrew_city_alias("Berlin"), None);
    }

    #[test]
    fn wmo_label_covers_common_buckets() {
        assert_eq!(wmo_label(0), "clear");
        assert_eq!(wmo_label(2), "partly cloudy");
        assert_eq!(wmo_label(61), "rain");
        assert_eq!(wmo_label(95), "thunderstorm");
        assert_eq!(wmo_label(9999), "unknown");
    }

    #[test]
    fn schemas_lists_four_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 4);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "wikipedia_search",
                "wikipedia_summary",
                "weather_current",
                "weather_forecast"
            ]
        );
    }
}
