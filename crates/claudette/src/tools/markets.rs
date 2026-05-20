//! Markets group — TradingView quotes. 1 tool.
//!
//! TradingView has no official public API — we hit the
//! `scanner.tradingview.com` endpoints that the TradingView web screener
//! itself uses. They're open (no auth) but sit in a ToS gray zone: the
//! public UI scrapes them, while scraping the website UI is explicitly
//! prohibited. At personal-agent volume the risk is near-zero; revisit
//! if Claudette ever goes multi-tenant.
//!
//! Sprint v0.6.0 (2026-05-21) decom dropped the broader Markets surface:
//! `tv_technical_rating`, `tv_search_symbol`, `tv_economic_calendar`, and
//! the three `vestige_*` Algorand tools all had zero positive invocations
//! in the 100-prompt sweep. `tv_get_quote` is the only quote/lookup tool
//! kept — it covers the user-visible "what's $TICKER doing" use case and
//! its built-in alias resolution (BTC → BINANCE:BTCUSDT, GOLD → TVC:GOLD,
//! etc.) subsumes the symbol-search tool for the common cases.
//!
//! Parent-module helpers used: `parse_json_input`, `extract_str`,
//! `external_http_client`.

use serde_json::{json, Value};

use super::{external_http_client, extract_str, parse_json_input};

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "tv_get_quote",
            "description": "TradingView price + % change for stock/crypto/forex. Bare ticker (BTC, AAPL) or qualified (BINANCE:BTCUSDT).",
            "parameters": {
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Ticker (bare or EXCHANGE:SYM)" },
                    "market": { "type": "string", "description": "america (default), crypto, forex, futures" }
                },
                "required": ["symbol"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "tv_get_quote" => run_tv_get_quote(input),
        _ => return None,
    };
    Some(result)
}

// ────── TradingView helpers ──────────────────────────────────────────────

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
         Try qualifying with an exchange prefix (e.g. NASDAQ:AAPL, BINANCE:BTCUSDT)."
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn schemas_lists_one_tool() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 1);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["tv_get_quote"]);
    }
}
