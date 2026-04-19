//! Markets group — TradingView + vestige.fi (Algorand ASAs). 7 tools.
//!
//! TradingView has no official public API — we hit the
//! `scanner.tradingview.com` endpoints that the TradingView web screener
//! itself uses. They're open (no auth) but sit in a ToS gray zone: the
//! public UI scrapes them, while scraping the website UI is explicitly
//! prohibited. At personal-agent volume the risk is near-zero; revisit
//! if Claudette ever goes multi-tenant.
//!
//! vestige.fi exposes a documented public REST API at `api.vestigelabs.org`
//! — no key needed for reasonable read use. Canonical aggregator for
//! Algorand DEX price data across Tinyman/Pact/Humble.
//!
//! **Vestige price denomination gotcha:** by default the API returns prices
//! in ALGO (asset id 0). We always pass `denominating_asset_id=31566704`
//! (USDC) so Claudette can report real USD numbers without further
//! conversion.
//!
//! Parent-module helpers used: parse_json_input, extract_str,
//! external_http_client, strip_html.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input, strip_html};

/// USDC ASA id on the Algorand mainnet. Used as the default denominating
/// asset so vestige prices come back in USD.
const VESTIGE_USDC_ASA_ID: i64 = 31566704;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "tv_get_quote",
                "description": "Get current price and % change for a stock/crypto/forex symbol via TradingView. Accepts bare tickers (BTC, AAPL) or qualified (BINANCE:BTCUSDT, NASDAQ:NVDA). Default market 'america'; use 'crypto' for coins.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Ticker — bare (BTC, AAPL) or with exchange (BINANCE:BTCUSDT)" },
                        "market": { "type": "string", "description": "Market: 'america' (default), 'crypto', 'forex', 'futures'" }
                    },
                    "required": ["symbol"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tv_technical_rating",
                "description": "Get TradingView's technical rating (strong_buy/buy/neutral/sell/strong_sell) for a symbol at a given interval. Accepts bare tickers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol":   { "type": "string", "description": "Ticker — bare (BTC, AAPL) or with exchange (BINANCE:BTCUSDT)" },
                        "interval": { "type": "string", "description": "Interval: '1m','5m','15m','1h','4h','1d','1W','1M' (default '1d')" },
                        "market":   { "type": "string", "description": "Market: 'america' (default), 'crypto', 'forex', 'futures'" }
                    },
                    "required": ["symbol"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tv_search_symbol",
                "description": "Search TradingView for a symbol by name or ticker. Returns top 5 hits with exchange.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Symbol or company name" }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "tv_economic_calendar",
                "description": "Get upcoming economic calendar events from TradingView (CPI, FOMC, NFP, etc).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "days_ahead": { "type": "number", "description": "Days forward from today (default 7, max 30)" },
                        "countries":  { "type": "string", "description": "Comma-separated country codes (default 'US')" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "vestige_asa_info",
                "description": "Get current USD/ALGO price and 24h change for an Algorand ASA by its asset id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "asa_id": { "type": "number", "description": "Algorand Standard Asset ID (e.g. 31566704 for USDC)" }
                    },
                    "required": ["asa_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "vestige_search_asa",
                "description": "Search vestige.fi for an Algorand ASA by ticker or name. Returns top 5 matches with ids.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Ticker or name (e.g. 'USDC', 'OPUL')" }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "vestige_top_movers",
                "description": "Get top Algorand ASA movers from vestige.fi ('gainers' or 'losers' by 24h change).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "direction": { "type": "string", "description": "'gainers' (default) or 'losers'" },
                        "limit":     { "type": "number", "description": "Number of results (default 5, max 20)" }
                    },
                    "required": []
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "tv_get_quote" => run_tv_get_quote(input),
        "tv_technical_rating" => run_tv_technical_rating(input),
        "tv_search_symbol" => run_tv_search_symbol(input),
        "tv_economic_calendar" => run_tv_economic_calendar(input),
        "vestige_asa_info" => run_vestige_asa_info(input),
        "vestige_search_asa" => run_vestige_search_asa(input),
        "vestige_top_movers" => run_vestige_top_movers(input),
        _ => return None,
    };
    Some(result)
}

// ────── TradingView helpers ──────────────────────────────────────────────

/// Map a `TradingView` interval string ("1m", "1h", "1d", "1W") to the suffix
/// the scanner API expects on column names like `Recommend.All|15`. Returns
/// an empty string for daily (the default, no suffix).
fn tv_interval_suffix(interval: &str) -> Result<&'static str, String> {
    match interval.trim() {
        "" | "1d" | "D" => Ok(""),
        "1m" => Ok("|1"),
        "5m" => Ok("|5"),
        "15m" => Ok("|15"),
        "30m" => Ok("|30"),
        "1h" | "60m" => Ok("|60"),
        "2h" | "120m" => Ok("|120"),
        "4h" | "240m" => Ok("|240"),
        "1W" | "W" => Ok("|1W"),
        "1M" | "M" => Ok("|1M"),
        other => Err(format!(
            "tv: unknown interval '{other}' — use 1m/5m/15m/30m/1h/4h/1d/1W/1M"
        )),
    }
}

/// Map a `TradingView` `Recommend.All` float score (−1.0 to 1.0) to a label.
/// Standard buckets used by the TV UI: ≥0.5 = `strong_buy`, ≥0.1 = buy, etc.
fn tv_rating_label(score: f64) -> &'static str {
    if score >= 0.5 {
        "strong_buy"
    } else if score >= 0.1 {
        "buy"
    } else if score > -0.1 {
        "neutral"
    } else if score > -0.5 {
        "sell"
    } else {
        "strong_sell"
    }
}

/// Normalise the user-visible market parameter to the path segment the
/// scanner API uses. Default: "america" (US stocks).
fn tv_market_path(market: Option<&str>) -> &'static str {
    match market.unwrap_or("america").to_lowercase().as_str() {
        "crypto" | "cryptos" => "crypto",
        "forex" | "fx" => "forex",
        "futures" | "futures_contracts" => "futures",
        _ => "america",
    }
}

/// Shared POST helper for `scanner.tradingview.com/{market}/scan`. Returns
/// the parsed `data` array (each entry is `{ s: symbol, d: [values...] }`).
fn tv_scan_request(market: &str, body: &Value) -> Result<Vec<Value>, String> {
    let url = format!("https://scanner.tradingview.com/{market}/scan");
    let client = external_http_client()?;
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .map_err(|e| format!("tv_scan: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "tv_scan: HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_scan: parse failed: {e}"))?;

    Ok(data
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Try to resolve a bare or prefixed symbol to one that `TradingView` accepts.
///
/// If the symbol already contains a colon (e.g. `BINANCE:BTCUSDT`), use it
/// as-is. Otherwise, try a list of common exchange prefixes in order. Returns
/// `(resolved_symbol, rows)` where `rows` contains the scan result.
///
/// Common aliases handled:
///  - Crypto: `BTC` → `BINANCE:BTCUSDT`, `ETH` → `BINANCE:ETHUSDT`, etc.
///  - US stocks: bare ticker → `NASDAQ:<T>`, `NYSE:<T>`
fn resolve_tv_symbol(
    raw_symbol: &str,
    market: &str,
    columns: &Value,
) -> Result<(String, Vec<Value>), String> {
    let raw = raw_symbol.trim().to_uppercase();

    // Well-known commodity/index/crypto aliases. Covers the user's watchlist
    // and common names the model might use in natural language.
    let sym = match raw.as_str() {
        // ── Commodities (TVC: symbols work on any market path) ──
        "GOLD" | "XAU" | "XAUUSD" => "TVC:GOLD".to_string(),
        "SILVER" | "XAG" | "XAGUSD" => "TVC:SILVER".to_string(),
        "OIL" | "USOIL" | "CRUDE" | "WTI" | "CL" => "TVC:USOIL".to_string(),
        "BRENT" | "UKOIL" => "TVC:UKOIL".to_string(),
        "NATGAS" | "NG" | "NATURALGAS" => "TVC:NATURALGAS".to_string(),
        // ── Indices ──
        "NASDAQ" | "NDX" | "NDQ" | "QQQ" => "NASDAQ:NDX".to_string(),
        "SPX" | "SP500" | "S&P500" | "S&P" => "SP:SPX".to_string(),
        "DJI" | "DOW" | "DOWJONES" => "DJ:DJI".to_string(),
        "DXY" | "DOLLAR" => "TVC:DXY".to_string(),
        "VIX" => "TVC:VIX".to_string(),
        // ── Crypto (major pairs) ──
        "BTCUSD" | "BTCUSDT" => "BINANCE:BTCUSDT".to_string(),
        "ETHUSD" | "ETHUSDT" => "BINANCE:ETHUSDT".to_string(),
        "ALGOUSDT" | "ALGOUSI" | "ALGO" => "BINANCE:ALGOUSDT".to_string(),
        "KASUSDT" | "KAS" | "KASPA" => "MEXC:KASUSDT".to_string(),
        "ICPUSD" | "ICPUSDT" | "ICP" => "COINBASE:ICPUSD".to_string(),
        "QNTUSDT" | "QNT" => "BINANCE:QNTUSDT".to_string(),
        "BTCXAI" => "MEXC:BTCXAIUSDT".to_string(),
        // ── Forex ──
        "EURUSD" => "FX:EURUSD".to_string(),
        "USDJPY" => "FX:USDJPY".to_string(),
        "GBPUSD" => "FX:GBPUSD".to_string(),
        _ => raw,
    };

    // Already qualified (has exchange prefix) — try once.
    if sym.contains(':') {
        let body = json!({
            "symbols": { "tickers": [&sym], "query": { "types": [] } },
            "columns": columns,
        });
        let rows = tv_scan_request(market, &body)?;
        if !rows.is_empty() {
            return Ok((sym, rows));
        }
        return Err(format!("tv: symbol '{sym}' not found on market '{market}'"));
    }

    // Crypto shorthand: BTC → BINANCE:BTCUSDT, ETH → BINANCE:ETHUSDT, etc.
    static CRYPTO_SUFFIXES: &[(&str, &str)] = &[
        ("BINANCE:", "USDT"),
        ("COINBASE:", "USD"),
        ("BINANCE:", "USD"),
    ];

    // US stock exchanges.
    static STOCK_PREFIXES: &[&str] = &["NASDAQ:", "NYSE:", "AMEX:"];

    let candidates: Vec<String> = if market == "crypto" {
        CRYPTO_SUFFIXES
            .iter()
            .map(|(prefix, suffix)| format!("{prefix}{sym}{suffix}"))
            .collect()
    } else {
        STOCK_PREFIXES
            .iter()
            .map(|prefix| format!("{prefix}{sym}"))
            .collect()
    };

    for candidate in &candidates {
        let body = json!({
            "symbols": { "tickers": [candidate], "query": { "types": [] } },
            "columns": columns,
        });
        if let Ok(rows) = tv_scan_request(market, &body) {
            if !rows.is_empty() {
                return Ok((candidate.clone(), rows));
            }
        }
    }

    Err(format!(
        "tv: symbol '{raw_symbol}' not found. Tried: {candidates:?}. \
         Use tv_search_symbol to find the correct exchange prefix."
    ))
}

// ────── TradingView handlers ─────────────────────────────────────────────

fn run_tv_get_quote(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_get_quote")?;
    let raw_symbol = extract_str(&v, "symbol", "tv_get_quote")?.to_string();
    let market = tv_market_path(v.get("market").and_then(Value::as_str));

    let columns = json!([
        "close",
        "change",
        "change_abs",
        "volume",
        "high",
        "low",
        "open"
    ]);
    let (symbol, rows) = resolve_tv_symbol(&raw_symbol, market, &columns)?;
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| format!("tv_get_quote: symbol '{symbol}' not found on market '{market}'"))?;

    let cells = row
        .get("d")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let get = |i: usize| cells.get(i).and_then(Value::as_f64);

    Ok(json!({
        "symbol": symbol,
        "market": market,
        "close": get(0),
        "change_pct": get(1),
        "change_abs": get(2),
        "volume": get(3),
        "high": get(4),
        "low": get(5),
        "open": get(6),
    })
    .to_string())
}

fn run_tv_technical_rating(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_technical_rating")?;
    let raw_symbol = extract_str(&v, "symbol", "tv_technical_rating")?.to_string();
    let interval = v.get("interval").and_then(Value::as_str).unwrap_or("1d");
    let suffix = tv_interval_suffix(interval)?;
    let market = tv_market_path(v.get("market").and_then(Value::as_str));

    // Build the three Recommend.* column names for the requested interval.
    let col_all = format!("Recommend.All{suffix}");
    let col_ma = format!("Recommend.MA{suffix}");
    let col_other = format!("Recommend.Other{suffix}");

    let columns = json!([col_all, col_ma, col_other]);
    let (symbol, rows) = resolve_tv_symbol(&raw_symbol, market, &columns)?;
    let row = rows.into_iter().next().ok_or_else(|| {
        format!("tv_technical_rating: symbol '{symbol}' not found on market '{market}'")
    })?;

    let cells = row
        .get("d")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let get = |i: usize| cells.get(i).and_then(Value::as_f64);
    let overall = get(0);
    let ma = get(1);
    let other = get(2);

    Ok(json!({
        "symbol": symbol,
        "market": market,
        "interval": interval,
        "overall_score": overall,
        "overall_rating": overall.map_or("", tv_rating_label),
        "moving_averages_score": ma,
        "moving_averages_rating": ma.map_or("", tv_rating_label),
        "oscillators_score": other,
        "oscillators_rating": other.map_or("", tv_rating_label),
    })
    .to_string())
}

fn run_tv_search_symbol(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_search_symbol")?;
    let query = extract_str(&v, "query", "tv_search_symbol")?;

    let client = external_http_client()?;
    let resp = client
        .get("https://symbol-search.tradingview.com/symbol_search/")
        .query(&[
            ("text", query),
            ("hl", "1"),
            ("exchange", ""),
            ("lang", "en"),
            ("type", ""),
            ("domain", "production"),
        ])
        .send()
        .map_err(|e| format!("tv_search_symbol: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("tv_search_symbol: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_search_symbol: parse failed: {e}"))?;

    // The endpoint returns either a top-level array or `{ "symbols": [...] }`
    // depending on the client; handle both shapes defensively.
    let raw = data
        .as_array()
        .cloned()
        .or_else(|| data.get("symbols").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    let results: Vec<Value> = raw
        .iter()
        .take(5)
        .map(|r| {
            let description = r
                .get("description")
                .and_then(Value::as_str)
                .map(strip_html)
                .unwrap_or_default();
            json!({
                "symbol": r.get("symbol").and_then(Value::as_str).unwrap_or(""),
                "description": description,
                "type": r.get("type").and_then(Value::as_str).unwrap_or(""),
                "exchange": r.get("exchange").and_then(Value::as_str).unwrap_or(""),
                "prefix": r.get("prefix").and_then(Value::as_str).unwrap_or(""),
                "country": r.get("country").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_tv_economic_calendar(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "tv_economic_calendar")?;
    let days_ahead = v
        .get("days_ahead")
        .and_then(Value::as_i64)
        .unwrap_or(7)
        .clamp(1, 30);
    let countries = v
        .get("countries")
        .and_then(Value::as_str)
        .unwrap_or("US")
        .to_string();

    let now = chrono::Utc::now();
    let end = now + chrono::Duration::days(days_ahead);
    let from = now.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
    let to = end.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();

    let client = external_http_client()?;
    // The economic-calendar subdomain enforces Origin/Referer checks at the
    // nginx layer — without these headers it returns 403 Forbidden. The
    // scanner subdomain does NOT enforce them, which is why scanner quotes
    // work but the calendar doesn't by default. Learned the hard way while
    // running the brain30_sprint9 test (2026-04-12).
    let resp = client
        .get("https://economic-calendar.tradingview.com/events")
        .query(&[
            ("from", from.as_str()),
            ("to", to.as_str()),
            ("countries", countries.as_str()),
        ])
        .header("Origin", "https://www.tradingview.com")
        .header("Referer", "https://www.tradingview.com/")
        .send()
        .map_err(|e| format!("tv_economic_calendar: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("tv_economic_calendar: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("tv_economic_calendar: parse failed: {e}"))?;

    let events = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let results: Vec<Value> = events
        .iter()
        .take(20)
        .map(|e| {
            json!({
                "title": e.get("title").and_then(Value::as_str).unwrap_or(""),
                "country": e.get("country").and_then(Value::as_str).unwrap_or(""),
                "date": e.get("date").and_then(Value::as_str).unwrap_or(""),
                "importance": e.get("importance").and_then(Value::as_i64).unwrap_or(0),
                "actual": e.get("actual").and_then(Value::as_f64),
                "forecast": e.get("forecast").and_then(Value::as_f64),
                "previous": e.get("previous").and_then(Value::as_f64),
                "period": e.get("period").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "from": from,
        "to": to,
        "countries": countries,
        "count": results.len(),
        "events": results,
    })
    .to_string())
}

// ────── vestige.fi (Algorand ASAs) ───────────────────────────────────────
//
// The real API lives at `api.vestigelabs.org`, NOT `api.vestige.fi` (the
// latter DNS-resolves but returns Cloudflare 1016 / "origin unreachable"
// — discovered via browser DevTools on the vestige.fi homepage, 2026-04-12).
// The full OpenAPI spec is at `/openapi.json` — use that as the source of
// truth if this starts drifting.

/// Resolve the vestige.fi API base URL. Honours `VESTIGE_API_BASE` env var
/// so the user can flip to a paid tier or alternate endpoint if the free
/// base URL ever changes.
fn vestige_base_url() -> String {
    std::env::var("VESTIGE_API_BASE").unwrap_or_else(|_| "https://api.vestigelabs.org".to_string())
}

/// Extract the common `/assets/list` asset shape into the Claudette-facing
/// JSON format. Pulls price/volume/market-cap fields defensively so minor
/// upstream schema drift doesn't break the parse.
fn vestige_asset_json(a: &Value) -> Value {
    json!({
        "asa_id": a.get("id").and_then(Value::as_i64),
        "name": a.get("name").and_then(Value::as_str).unwrap_or(""),
        "ticker": a.get("ticker").and_then(Value::as_str).unwrap_or(""),
        "rank": a.get("rank").and_then(Value::as_i64),
        "price_usd": a.get("price").and_then(Value::as_f64),
        "price_24h_ago_usd": a.get("price1d").and_then(Value::as_f64),
        "price_7d_ago_usd": a.get("price7d").and_then(Value::as_f64),
        "change_24h_pct": calculate_change_pct(
            a.get("price").and_then(Value::as_f64),
            a.get("price1d").and_then(Value::as_f64),
        ),
        "volume_24h_usd": a.get("volume1d").and_then(Value::as_f64),
        "market_cap_usd": a.get("market_cap").and_then(Value::as_f64),
        "tvl_usd": a.get("tvl").and_then(Value::as_f64),
        "confidence": a.get("confidence").and_then(Value::as_f64),
        "total_supply": a.get("total_supply").and_then(Value::as_f64),
    })
}

/// Compute a percentage change from `old` to `new`, returning `None` if
/// either value is missing or `old` is zero.
fn calculate_change_pct(new: Option<f64>, old: Option<f64>) -> Option<f64> {
    let (n, o) = (new?, old?);
    if o.abs() < f64::EPSILON {
        return None;
    }
    Some((n - o) / o * 100.0)
}

fn run_vestige_asa_info(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_asa_info")?;
    let asa_id = v
        .get("asa_id")
        .and_then(Value::as_i64)
        .ok_or("vestige_asa_info: missing or non-numeric 'asa_id'")?;

    let base = vestige_base_url();
    // /assets/list with an asset_ids filter returns the same enriched shape
    // as the paginated listing (price, volume, market_cap, etc.), which is
    // what we want for a single-asset lookup.
    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let asa_id_str = asa_id.to_string();
    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/list"))
        .query(&[
            ("network_id", "0"),
            ("asset_ids", asa_id_str.as_str()),
            ("denominating_asset_id", denominating.as_str()),
        ])
        .send()
        .map_err(|e| format!("vestige_asa_info: request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("vestige_asa_info: HTTP {status}"));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_asa_info: parse failed: {e}"))?;

    let first = data
        .get("results")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .ok_or_else(|| format!("vestige_asa_info: no ASA with id {asa_id}"))?;

    Ok(vestige_asset_json(first).to_string())
}

fn run_vestige_search_asa(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_search_asa")?;
    let query = extract_str(&v, "query", "vestige_search_asa")?;
    let base = vestige_base_url();

    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/search"))
        .query(&[
            ("query", query),
            ("network_id", "0"),
            ("denominating_asset_id", denominating.as_str()),
            ("limit", "5"),
        ])
        .send()
        .map_err(|e| format!("vestige_search_asa: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("vestige_search_asa: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_search_asa: parse failed: {e}"))?;

    let raw = data
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let results: Vec<Value> = raw.iter().take(5).map(vestige_asset_json).collect();

    Ok(json!({
        "query": query,
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

fn run_vestige_top_movers(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "vestige_top_movers")?;
    let direction = v
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("gainers")
        .to_lowercase();
    let limit = v
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(5)
        .clamp(1, 20);

    // Top movers by 24h price change. Vestige exposes price1d as "price
    // 24h ago" so we sort by current / price1d. Since there's no direct
    // sort param for "% change 24h", we sort by volume1d (which correlates
    // with movement) and compute change in post-processing. This trades a
    // little precision for a simpler call.
    let base = vestige_base_url();
    let denominating = VESTIGE_USDC_ASA_ID.to_string();
    let fetch_limit = (limit * 6).clamp(20, 100).to_string();

    let client = external_http_client()?;
    let resp = client
        .get(format!("{base}/assets/list"))
        .query(&[
            ("network_id", "0"),
            ("denominating_asset_id", denominating.as_str()),
            ("order_by", "volume1d"),
            ("order_dir", "desc"),
            ("limit", fetch_limit.as_str()),
            // Filter out low-confidence spam tokens.
            ("tvl__gt", "10000"),
        ])
        .send()
        .map_err(|e| format!("vestige_top_movers: request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("vestige_top_movers: HTTP {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .map_err(|e| format!("vestige_top_movers: parse failed: {e}"))?;

    let raw = data
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Compute % change per asset, then sort.
    let mut scored: Vec<(f64, Value)> = raw
        .iter()
        .filter_map(|a| {
            let now = a.get("price").and_then(Value::as_f64)?;
            let then = a.get("price1d").and_then(Value::as_f64)?;
            if then.abs() < f64::EPSILON {
                return None;
            }
            let pct = (now - then) / then * 100.0;
            if !pct.is_finite() {
                return None;
            }
            Some((pct, a.clone()))
        })
        .collect();

    // Sort by % change: gainers → descending, losers → ascending.
    if direction == "losers" {
        scored.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        scored.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
    }

    let results: Vec<Value> = scored
        .into_iter()
        .take(limit as usize)
        .map(|(_, a)| vestige_asset_json(&a))
        .collect();

    Ok(json!({
        "direction": if direction == "losers" { "losers" } else { "gainers" },
        "count": results.len(),
        "results": results,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tv_rating_label_buckets() {
        assert_eq!(tv_rating_label(0.8), "strong_buy");
        assert_eq!(tv_rating_label(0.3), "buy");
        assert_eq!(tv_rating_label(0.05), "neutral");
        assert_eq!(tv_rating_label(-0.05), "neutral");
        assert_eq!(tv_rating_label(-0.3), "sell");
        assert_eq!(tv_rating_label(-0.8), "strong_sell");
    }

    #[test]
    fn tv_interval_suffix_known() {
        assert_eq!(tv_interval_suffix("1d").unwrap(), "");
        assert_eq!(tv_interval_suffix("").unwrap(), "");
        assert_eq!(tv_interval_suffix("1m").unwrap(), "|1");
        assert_eq!(tv_interval_suffix("15m").unwrap(), "|15");
        assert_eq!(tv_interval_suffix("1h").unwrap(), "|60");
        assert_eq!(tv_interval_suffix("4h").unwrap(), "|240");
        assert_eq!(tv_interval_suffix("1W").unwrap(), "|1W");
        assert!(tv_interval_suffix("bogus").is_err());
    }

    #[test]
    fn tv_market_path_defaults_to_america() {
        assert_eq!(tv_market_path(None), "america");
        assert_eq!(tv_market_path(Some("america")), "america");
        assert_eq!(tv_market_path(Some("crypto")), "crypto");
        assert_eq!(tv_market_path(Some("CRYPTO")), "crypto");
        assert_eq!(tv_market_path(Some("forex")), "forex");
        assert_eq!(tv_market_path(Some("futures")), "futures");
        assert_eq!(tv_market_path(Some("klingon")), "america");
    }

    #[test]
    fn tv_get_quote_rejects_missing_symbol() {
        let err = run_tv_get_quote("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn tv_technical_rating_rejects_missing_symbol() {
        let err = run_tv_technical_rating("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn tv_technical_rating_rejects_bad_interval() {
        let err =
            run_tv_technical_rating(r#"{"symbol":"NASDAQ:NVDA","interval":"nope"}"#).unwrap_err();
        assert!(err.contains("unknown interval"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_qualified_returns_as_is_on_failure() {
        // Qualified symbol (has colon) skips auto-resolution and gives a clear error.
        let err = resolve_tv_symbol("FAKE:NOSUCH", "america", &json!(["close"])).unwrap_err();
        assert!(err.contains("FAKE:NOSUCH"), "got: {err}");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_bare_crypto_tries_binance() {
        // Bare crypto symbol should try BINANCE:BTCUSDT etc. Won't succeed
        // without network, but the error should mention the candidates.
        let err = resolve_tv_symbol("FAKECOIN", "crypto", &json!(["close"])).unwrap_err();
        assert!(err.contains("BINANCE:FAKECOINUSDT"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_bare_stock_tries_nasdaq() {
        let err = resolve_tv_symbol("FAKESTOCK", "america", &json!(["close"])).unwrap_err();
        assert!(err.contains("NASDAQ:FAKESTOCK"), "got: {err}");
    }

    #[test]
    fn resolve_tv_symbol_commodity_aliases() {
        // Commodity aliases resolve to qualified symbols (which contain ':').
        // May succeed on the network (if the symbol is valid) or fail — either
        // way the resolved symbol should be the aliased one.
        match resolve_tv_symbol("GOLD", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("TVC:GOLD"), "got: {sym}"),
            Err(e) => assert!(e.contains("TVC:GOLD"), "got: {e}"),
        }
        match resolve_tv_symbol("OIL", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("TVC:USOIL"), "got: {sym}"),
            Err(e) => assert!(e.contains("TVC:USOIL"), "got: {e}"),
        }
        match resolve_tv_symbol("NASDAQ", "america", &json!(["close"])) {
            Ok((sym, _)) => assert!(sym.contains("NASDAQ:NDX"), "got: {sym}"),
            Err(e) => assert!(e.contains("NASDAQ:NDX"), "got: {e}"),
        }
    }

    #[test]
    fn tv_search_rejects_missing_query() {
        let err = run_tv_search_symbol("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn vestige_asa_info_rejects_missing_id() {
        let err = run_vestige_asa_info("{}").unwrap_err();
        assert!(err.contains("asa_id"), "got: {err}");
    }

    #[test]
    fn vestige_asa_info_rejects_non_numeric_id() {
        let err = run_vestige_asa_info(r#"{"asa_id":"USDC"}"#).unwrap_err();
        assert!(err.contains("asa_id"), "got: {err}");
    }

    #[test]
    fn vestige_search_rejects_missing_query() {
        let err = run_vestige_search_asa("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn vestige_base_url_default() {
        // Just check the default is stable. Env-var override test would
        // race with other tests so we skip it.
        let base = vestige_base_url();
        assert!(base.starts_with("http"));
        assert!(base.contains("vestige"));
    }

    #[test]
    fn schemas_lists_seven_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 7);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "tv_get_quote",
                "tv_technical_rating",
                "tv_search_symbol",
                "tv_economic_calendar",
                "vestige_asa_info",
                "vestige_search_asa",
                "vestige_top_movers",
            ]
        );
    }
}
