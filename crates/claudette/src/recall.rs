//! Cross-session semantic recall — long-term memory the agent can query
//! across sessions.
//!
//! Every text message (user + assistant) is embedded with `nomic-embed-text`
//! via Ollama and stored in `~/.claudette/recall.sqlite`. At query time the
//! query text is embedded the same way, and the top-k rows by cosine
//! similarity are returned. Tool calls and tool results are intentionally
//! NOT indexed (too noisy, would balloon the index size).
//!
//! Lazy install: the first call discovers whether Ollama already has the
//! embed model and pulls it (`POST /api/pull`) on miss with a status line
//! to stderr. Subsequent calls skip the probe. ~270MB / 768 dims / MIT.
//!
//! Storage caps at 50_000 rows with FIFO eviction (oldest id first).
//! ~3KB/row → ~150MB ceiling.
//!
//! The embed call is wrapped behind [`Embedder`] so the live tests can
//! inject a deterministic mock without standing up Ollama. Production code
//! goes through [`global_index`] / [`global_query`], which lazy-init a
//! process-wide [`RecallStore`] backed by either [`OllamaEmbedder`] (the
//! default) or [`OpenAICompatEmbedder`] (when `CLAUDETTE_OPENAI_COMPAT=1`,
//! e.g. against LM Studio's `/v1/embeddings`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::api::{resolve_ollama_url, resolve_openai_compat};

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

/// Which side of the conversation a snippet came from. The DB stores this
/// as TEXT (`"user"` / `"assistant"`); kept as an enum at the API edge so
/// callers can't write nonsense values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// One scored hit returned by [`RecallStore::query`].
#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub ts: String,
    pub role: Role,
    pub snippet: String,
    /// Cosine similarity in [-1, 1]. Higher is more similar.
    pub score: f32,
}

/// Internal heap entry. Wraps `RecallHit` with an `Ord` impl over the
/// score, since `f32` is only `PartialOrd`. NaN sinks to the smallest so
/// it gets evicted from the top-k heap first.
#[derive(Debug, Clone)]
struct ScoredHit {
    score: f32,
    hit: RecallHit,
}

impl PartialEq for ScoredHit {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for ScoredHit {}

impl PartialOrd for ScoredHit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredHit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // NaN-tolerant: treat NaN as smaller than everything so it
        // evicts first from the heap.
        match self.score.partial_cmp(&other.score) {
            Some(o) => o,
            None => {
                if self.score.is_nan() && other.score.is_nan() {
                    std::cmp::Ordering::Equal
                } else if self.score.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
        }
    }
}

/// Embedding-model abstraction. Production = [`OllamaEmbedder`]; tests
/// inject a deterministic mock so they don't need a live Ollama.
pub trait Embedder: Send {
    /// Embed a single text and return the vector. Dimensionality is
    /// fixed per implementation — the store records it on first insert
    /// and rejects mismatched future inserts.
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, String>;
}

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/// Hard cap on stored rows. FIFO-evicted on insert overflow. ~3KB/row at
/// 768 dims → ~150MB ceiling. Decision rationale lives in
/// `project_claudette_recall_feature_2026-05-08.md`.
pub const RECALL_ROW_CAP: usize = 50_000;

/// Default embed model. Ollama accepts either `nomic-embed-text` or
/// `nomic-embed-text:latest`; the bare form is what `ollama pull` prints
/// in its progress bar so we use that for consistency.
pub const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";

// ────────────────────────────────────────────────────────────────────────────
// Path resolution
// ────────────────────────────────────────────────────────────────────────────

/// Resolve the recall DB path. `CLAUDETTE_RECALL_DB` overrides for tests
/// and bring-your-own-path setups; otherwise `~/.claudette/recall.sqlite`.
#[must_use]
pub fn default_recall_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDETTE_RECALL_DB") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::env_config::home_dir()
        .join(".claudette")
        .join("recall.sqlite")
}

// ────────────────────────────────────────────────────────────────────────────
// BLOB encode/decode for f32 vectors (little-endian)
// ────────────────────────────────────────────────────────────────────────────

#[must_use]
pub fn encode_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn decode_vec(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(bytes.len() / 4);
    decode_vec_into(bytes, &mut out)?;
    Ok(out)
}

/// Decode a vector into a pre-allocated buffer. `dst` is cleared first;
/// the caller picks the capacity so this can be reused across rows in a
/// scan without re-allocating 50K times per query.
pub fn decode_vec_into(bytes: &[u8], dst: &mut Vec<f32>) -> Result<(), String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "recall: BLOB length {} is not a multiple of 4 — corrupt vector",
            bytes.len()
        ));
    }
    dst.clear();
    dst.reserve(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().expect("chunks_exact yields 4-byte slices");
        dst.push(f32::from_le_bytes(arr));
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Cosine similarity
// ────────────────────────────────────────────────────────────────────────────

/// Cosine similarity. Returns `0.0` when either vector is the zero vector
/// (instead of NaN); callers treat zero as "no match" which is what we
/// want.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

// ────────────────────────────────────────────────────────────────────────────
// RecallStore — sqlite + embedder
// ────────────────────────────────────────────────────────────────────────────

pub struct RecallStore {
    conn: Connection,
    embedder: Box<dyn Embedder>,
    cap: usize,
}

impl RecallStore {
    /// Open a store at `path`, creating the schema if missing.
    pub fn open(path: impl AsRef<Path>, embedder: Box<dyn Embedder>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("recall: create_dir_all {}: {e}", parent.display()))?;
        }
        let conn =
            Connection::open(path).map_err(|e| format!("recall: open {}: {e}", path.display()))?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn,
            embedder,
            cap: RECALL_ROW_CAP,
        })
    }

    /// Open an in-memory store. Used by the unit tests.
    pub fn open_in_memory(embedder: Box<dyn Embedder>) -> Result<Self, String> {
        let conn =
            Connection::open_in_memory().map_err(|e| format!("recall: open in-memory: {e}"))?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn,
            embedder,
            cap: RECALL_ROW_CAP,
        })
    }

    /// Override the row cap. Tests use this to exercise FIFO eviction
    /// without inserting 50k rows.
    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    fn init_schema(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS recall (
                id      INTEGER PRIMARY KEY,
                ts      TEXT    NOT NULL,
                role    TEXT    NOT NULL,
                snippet TEXT    NOT NULL,
                vec     BLOB    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_recall_ts ON recall(ts);",
        )
        .map_err(|e| format!("recall: init schema: {e}"))
    }

    /// Embed `snippet` and insert it. No-op for empty/whitespace snippets.
    /// Trims to 8KB to keep the DB bounded against runaway long messages
    /// (image OCR transcripts, paste storms, etc).
    pub fn index(&mut self, role: Role, snippet: &str) -> Result<(), String> {
        let trimmed = snippet.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        // Cap stored snippet at 8KB. The embedder still sees the trimmed
        // version, but extreme inputs (think: the assistant pasting a
        // 100KB log) shouldn't bloat the DB.
        let stored: &str = if trimmed.len() > 8 * 1024 {
            // Clamp to the largest char boundary ≤ 8 KB. A raw byte slice at
            // `8*1024` panics when that index lands inside a multibyte glyph
            // — exactly the >8 KB non-ASCII inputs this cap targets (CJK,
            // emoji, image-OCR transcripts) — and `panic="abort"` turns that
            // into a whole-process abort. (roast 2026-06-02 / issue #26 §C)
            let mut end = 8 * 1024;
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            &trimmed[..end]
        } else {
            trimmed
        };
        let vec = self.embedder.embed(trimmed)?;
        let ts = chrono::Utc::now().to_rfc3339();
        let blob = encode_vec(&vec);
        self.conn
            .execute(
                "INSERT INTO recall (ts, role, snippet, vec) VALUES (?1, ?2, ?3, ?4)",
                params![ts, role.as_str(), stored, blob],
            )
            .map_err(|e| format!("recall: insert: {e}"))?;
        self.evict_to_cap()?;
        Ok(())
    }

    /// Embed `query` and return the top `k` rows by cosine similarity,
    /// sorted descending by score.
    ///
    /// **Hot-loop optimizations (2026-05-15):** the previous implementation
    /// called [`cosine_similarity`] per row, which recomputed the query's
    /// norm on every row, and decoded each stored vector into a fresh
    /// `Vec<f32>` allocation. At the 50 K row cap that's 50 K wasted
    /// `qvec` norm passes and 50 K allocations per query. This rewrite:
    /// 1. computes `||q||` once outside the loop,
    /// 2. reuses one decode buffer across all rows,
    /// 3. fuses the dot-product and stored-vector norm into a single pass
    ///    over the row,
    /// 4. keeps only the running top-k via a min-heap so we don't sort the
    ///    full scored list.
    ///
    /// Sub-linear search (HNSW / sqlite-vec) is still the right answer
    /// past ~100K rows but it adds a heavy dependency. The audit flagged
    /// the brute-force scan as "borderline acceptable" at 50 K — these
    /// constant-factor wins move it back into "comfortable" without
    /// changing the storage model.
    pub fn query(&mut self, query: &str, k: usize) -> Result<Vec<RecallHit>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let qvec = self.embedder.embed(trimmed)?;

        // Norm of q is constant across the scan — hoist it out.
        let mut qnorm_sq = 0.0_f32;
        for &x in &qvec {
            qnorm_sq = x.mul_add(x, qnorm_sq);
        }
        if qnorm_sq <= 0.0 {
            return Ok(Vec::new());
        }
        let qnorm = qnorm_sq.sqrt();

        let mut stmt = self
            .conn
            .prepare("SELECT ts, role, snippet, vec FROM recall")
            .map_err(|e| format!("recall: prepare select: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                let ts: String = row.get(0)?;
                let role: String = row.get(1)?;
                let snippet: String = row.get(2)?;
                let vec_blob: Vec<u8> = row.get(3)?;
                Ok((ts, role, snippet, vec_blob))
            })
            .map_err(|e| format!("recall: query_map: {e}"))?;

        // Single buffer reused across rows to avoid 50K allocations on a
        // full-table scan.
        let mut vbuf: Vec<f32> = Vec::with_capacity(qvec.len());
        // Min-heap of (-score, hit) so the smallest score is at the top
        // and gets bumped when a better one arrives. `Reverse` keeps the
        // ordering inverted on `f32` (NaN/partial_cmp tolerance built in).
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut heap: BinaryHeap<Reverse<ScoredHit>> = BinaryHeap::with_capacity(k + 1);
        for row in rows {
            let (ts, role_str, snippet, blob) =
                row.map_err(|e| format!("recall: row error: {e}"))?;
            let Some(role) = Role::parse(&role_str) else {
                continue; // skip rows with unknown roles
            };
            if decode_vec_into(&blob, &mut vbuf).is_err() {
                continue; // skip corrupt rows
            }
            if vbuf.len() != qvec.len() {
                continue; // dim mismatch — old rows from a different model
            }
            // Fused dot + ||b||² — one pass, exploits FMA on x86.
            let mut dot = 0.0_f32;
            let mut bnorm_sq = 0.0_f32;
            for (qx, &bx) in qvec.iter().zip(&vbuf) {
                dot = qx.mul_add(bx, dot);
                bnorm_sq = bx.mul_add(bx, bnorm_sq);
            }
            let denom = qnorm * bnorm_sq.sqrt();
            let score = if denom == 0.0 { 0.0 } else { dot / denom };

            heap.push(Reverse(ScoredHit {
                score,
                hit: RecallHit {
                    ts,
                    role,
                    snippet,
                    score,
                },
            }));
            if heap.len() > k {
                heap.pop();
            }
        }

        // Heap is sorted ascending by score; drain and reverse so the
        // caller sees descending order.
        let mut hits: Vec<RecallHit> = heap.into_iter().map(|Reverse(s)| s.hit).collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(hits)
    }

    /// Total row count. Useful for `/status`-style reporting.
    pub fn count(&self) -> Result<usize, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM recall", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as usize)
            .map_err(|e| format!("recall: count: {e}"))
    }

    fn evict_to_cap(&mut self) -> Result<(), String> {
        let n = self.count()?;
        if n <= self.cap {
            return Ok(());
        }
        let to_remove = n - self.cap;
        // Oldest first by id (which is monotonic — INTEGER PRIMARY KEY
        // without AUTOINCREMENT still increments, and our inserts are
        // single-threaded behind the global Mutex).
        // SQLite expects a signed integer; clamp at i64::MAX (we will
        // never realistically have more than 9e18 rows to evict, but the
        // cast lint fires on the unchecked direction).
        let to_remove_i64 = i64::try_from(to_remove).unwrap_or(i64::MAX);
        self.conn
            .execute(
                "DELETE FROM recall WHERE id IN (SELECT id FROM recall ORDER BY id ASC LIMIT ?1)",
                params![to_remove_i64],
            )
            .map_err(|e| format!("recall: evict: {e}"))?;
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// OllamaEmbedder — production embedder backed by /api/embeddings
// ────────────────────────────────────────────────────────────────────────────

pub struct OllamaEmbedder {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
    /// Latch: once we've confirmed the model is pullable on this process,
    /// don't probe again.
    ready: bool,
}

impl OllamaEmbedder {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            // Embed calls themselves are fast (<100ms) but the first call
            // on a cold model triggers a load that can take ~2s on CPU.
            // Pulls are bounded by the separate ensure_model branch.
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("recall: build http client: {e}"))?;
        let model = std::env::var("CLAUDETTE_RECALL_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string());
        Ok(Self {
            client,
            base_url: resolve_ollama_url(),
            model,
            ready: false,
        })
    }

    /// Make sure Ollama has the model loaded. POSTs to `/api/show`; if
    /// the model is missing, runs `/api/pull` (stream:false) with a
    /// status line on stderr so the user knows the ~270MB fetch is happening.
    fn ensure_model(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        // /api/show — cheapest probe; returns 404 when the model isn't
        // present locally.
        let show_url = format!("{}/api/show", self.base_url);
        let resp = self
            .client
            .post(&show_url)
            .json(&json!({ "name": self.model }))
            .send()
            .map_err(|e| {
                format!(
                    "recall: cannot reach Ollama at {} ({e}). Start it with `ollama serve`.",
                    self.base_url
                )
            })?;

        if resp.status().is_success() {
            self.ready = true;
            return Ok(());
        }

        // Anything other than 404 is a hard error — probably a config issue
        // we shouldn't paper over by trying to pull.
        if resp.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(format!(
                "recall: /api/show returned {} for {}",
                resp.status(),
                self.model
            ));
        }

        // Lazy install path — print a status line so the user knows why
        // the next call is taking a while.
        eprintln!(
            "{} pulling embed model {} (~270MB, one-time) ...",
            crate::theme::SAVE,
            crate::theme::accent(&self.model)
        );

        let pull_url = format!("{}/api/pull", self.base_url);
        let pull_resp = self
            .client
            // Pulls take real time — extend timeout for this call only.
            .post(&pull_url)
            .timeout(std::time::Duration::from_secs(600))
            .json(&json!({ "name": self.model, "stream": false }))
            .send()
            .map_err(|e| format!("recall: /api/pull request failed: {e}"))?;

        if !pull_resp.status().is_success() {
            return Err(format!(
                "recall: /api/pull returned {} for {} — try `ollama pull {}` manually",
                pull_resp.status(),
                self.model,
                self.model
            ));
        }

        eprintln!(
            "{} {} ready",
            crate::theme::ok(crate::theme::OK_GLYPH),
            crate::theme::ok(&self.model)
        );
        self.ready = true;
        Ok(())
    }
}

impl Embedder for OllamaEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, String> {
        self.ensure_model()?;
        let url = format!("{}/api/embeddings", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&json!({ "model": self.model, "prompt": text }))
            .send()
            .map_err(|e| format!("recall: /api/embeddings request: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("recall: /api/embeddings HTTP {}", resp.status()));
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("recall: /api/embeddings parse: {e}"))?;
        parse_ollama_embedding(&body)
    }
}

/// Parse the Ollama-native `/api/embeddings` response shape:
/// `{ "embedding": [f32, …] }`.
fn parse_ollama_embedding(body: &Value) -> Result<Vec<f32>, String> {
    let arr = body
        .get("embedding")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("recall: response missing 'embedding': {body}"))?;
    json_array_to_f32s(arr)
}

/// Parse the OpenAI-compat `/v1/embeddings` response shape:
/// `{ "data": [ { "embedding": [f32, …] }, … ] }`. Only the first element
/// is used since we only ever embed one input per request.
fn parse_openai_compat_embedding(body: &Value) -> Result<Vec<f32>, String> {
    let arr = body
        .get("data")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .and_then(|d| d.get("embedding"))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("recall: response missing 'data[0].embedding': {body}"))?;
    json_array_to_f32s(arr)
}

/// Convert a JSON array of numbers into `Vec<f32>`. Shared by both the
/// Ollama-native and OpenAI-compat response parsers.
fn json_array_to_f32s(arr: &[Value]) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let f = v
            .as_f64()
            .ok_or_else(|| "recall: non-numeric value in 'embedding'".to_string())?;
        out.push(f as f32);
    }
    if out.is_empty() {
        return Err("recall: empty embedding returned".to_string());
    }
    Ok(out)
}

// ────────────────────────────────────────────────────────────────────────────
// OpenAICompatEmbedder — production embedder backed by /v1/embeddings
// ────────────────────────────────────────────────────────────────────────────
//
// Used when CLAUDETTE_OPENAI_COMPAT=1, e.g. against LM Studio. Unlike
// OllamaEmbedder, no /api/show probe and no /api/pull auto-install — LM
// Studio expects the user to load the embed model ahead of time from its
// "Local Server" tab. A failed embed surfaces a clear hint to do so.

pub struct OpenAICompatEmbedder {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
}

impl OpenAICompatEmbedder {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("recall: build http client: {e}"))?;
        let model = std::env::var("CLAUDETTE_RECALL_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string());
        Ok(Self {
            client,
            base_url: resolve_ollama_url(),
            model,
        })
    }
}

impl Embedder for OpenAICompatEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&json!({ "model": self.model, "input": text }))
            .send()
            .map_err(|e| {
                format!(
                    "recall: cannot reach OpenAI-compat server at {} ({e}). \
                     Is LM Studio running on this port?",
                    self.base_url
                )
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "recall: /v1/embeddings HTTP {status} — load `{}` in LM Studio's \
                 Local Server tab (or set CLAUDETTE_RECALL_MODEL to a model id you have loaded). \
                 Body: {body}",
                self.model
            ));
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("recall: /v1/embeddings parse: {e}"))?;
        parse_openai_compat_embedding(&body)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Process-wide singleton
// ────────────────────────────────────────────────────────────────────────────

fn store_cell() -> &'static Mutex<Option<RecallStore>> {
    static CELL: OnceLock<Mutex<Option<RecallStore>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn lock_store() -> Result<MutexGuard<'static, Option<RecallStore>>, String> {
    store_cell()
        .lock()
        .map_err(|e| format!("recall: store lock poisoned: {e}"))
}

fn ensure_store(guard: &mut MutexGuard<'static, Option<RecallStore>>) -> Result<(), String> {
    if guard.is_some() {
        return Ok(());
    }
    let embedder: Box<dyn Embedder> = if resolve_openai_compat() {
        Box::new(OpenAICompatEmbedder::new()?)
    } else {
        Box::new(OllamaEmbedder::new()?)
    };
    let store = RecallStore::open(default_recall_db_path(), embedder)?;
    **guard = Some(store);
    Ok(())
}

/// Reset the global store. ONLY for tests — production code should never
/// call this. Drops the existing store (if any), so the next `global_*`
/// call lazy-inits afresh.
#[cfg(test)]
pub fn reset_global() {
    if let Ok(mut guard) = lock_store() {
        *guard = None;
    }
}

/// Install a custom store as the global. ONLY for tests — production code
/// should never call this. Used to inject the in-memory mock store.
#[cfg(test)]
pub fn install_global_for_test(store: RecallStore) {
    if let Ok(mut guard) = lock_store() {
        *guard = Some(store);
    }
}

/// Index `snippet` under `role`. Lazy-inits the global store on first call.
/// No-op for empty/whitespace snippets. Errors are returned to the caller —
/// the post-turn indexing hook turns them into a single warn line so a
/// transient Ollama outage doesn't break the REPL.
pub fn global_index(role: Role, snippet: &str) -> Result<(), String> {
    let mut guard = lock_store()?;
    ensure_store(&mut guard)?;
    guard
        .as_mut()
        .expect("ensure_store left store None")
        .index(role, snippet)
}

/// Run a top-`k` recall query against the global store. Lazy-inits on
/// first call.
pub fn global_query(query: &str, k: usize) -> Result<Vec<RecallHit>, String> {
    let mut guard = lock_store()?;
    ensure_store(&mut guard)?;
    guard
        .as_mut()
        .expect("ensure_store left store None")
        .query(query, k)
}

/// Pre-flight the recall embedder: do a tiny embed call and discard the
/// result. Lazy-inits the store on first use. Returns the upstream embed
/// error string on failure (e.g. LM Studio's "No models loaded" 400),
/// otherwise `Ok(())`.
///
/// The REPL and TUI call this once at startup so the user discovers a
/// missing embed model BEFORE their first turn, instead of after, with
/// noise on every subsequent turn until the sticky-disable layer kicks in.
pub fn probe() -> Result<(), String> {
    let mut guard = lock_store()?;
    ensure_store(&mut guard)?;
    guard
        .as_mut()
        .expect("ensure_store left store None")
        .embedder
        .embed("probe")
        .map(|_| ())
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic test embedder: hashes the input into a small fixed-dim
    /// vector. Two calls with identical text produce identical vectors;
    /// different text produces different vectors. Good enough for ranking
    /// roundtrip tests without a live Ollama.
    struct HashEmbedder {
        dim: usize,
    }

    impl HashEmbedder {
        fn new() -> Self {
            Self { dim: 8 }
        }
    }

    impl Embedder for HashEmbedder {
        fn embed(&mut self, text: &str) -> Result<Vec<f32>, String> {
            // Bag-of-words style: each char contributes to a bucket.
            // Sufficient signal for "matches my prior message" tests.
            let mut v = vec![0.0_f32; self.dim];
            for ch in text.chars() {
                let bucket = (ch as usize) % self.dim;
                v[bucket] += 1.0;
            }
            // Normalize so cosine equals dot product.
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            Ok(v)
        }
    }

    /// Embedder that always returns a fixed vector — for cap/eviction
    /// tests where we don't care about ranking quality.
    struct ConstEmbedder;
    impl Embedder for ConstEmbedder {
        fn embed(&mut self, _text: &str) -> Result<Vec<f32>, String> {
            Ok(vec![1.0, 0.0, 0.0, 0.0])
        }
    }

    /// Embedder that always fails — for probe-failure tests. Mimics the
    /// shape of LM Studio's "No models loaded" 400 response.
    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn embed(&mut self, _text: &str) -> Result<Vec<f32>, String> {
            Err(
                "recall: /v1/embeddings HTTP 400 — load `nomic-embed-text` in LM Studio's \
                 Local Server tab"
                    .to_string(),
            )
        }
    }

    #[test]
    fn embedder_failure_propagates_as_error_string() {
        // This is what `recall::probe` relies on: a failing embed must
        // surface the upstream error message intact, not swallow it. Pins
        // the contract since startup-probe diagnostics depend on the
        // user being able to read the LM Studio hint.
        let mut e = FailingEmbedder;
        let err = e.embed("probe").expect_err("FailingEmbedder must fail");
        assert!(err.contains("Local Server tab"), "got: {err}");
    }

    #[test]
    fn probe_through_store_returns_err_on_embedder_failure() {
        // Exercise the same code path `recall::probe` takes through the
        // store layer: ensure_store → embedder.embed → discard the vector.
        // Uses an in-memory store so we don't touch the global singleton.
        let mut store = RecallStore::open_in_memory(Box::new(FailingEmbedder)).expect("open");
        let err = store
            .embedder
            .embed("probe")
            .expect_err("FailingEmbedder must fail");
        assert!(err.contains("HTTP 400"), "got: {err}");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let v = vec![0.0, 1.0, -1.5, std::f32::consts::PI, f32::EPSILON];
        let bytes = encode_vec(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        let back = decode_vec(&bytes).expect("decode");
        assert_eq!(back, v);
    }

    #[test]
    fn decode_rejects_misaligned_bytes() {
        let err = decode_vec(&[1, 2, 3]).expect_err("should reject 3-byte input");
        assert!(err.contains("multiple of 4"), "got: {err}");
    }

    #[test]
    fn cosine_handles_zero_vectors() {
        // Don't NaN out on zero norm — return exactly 0.
        assert!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]).abs() < 1e-9);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 0.0]).abs() < 1e-9);
    }

    #[test]
    fn cosine_is_one_for_identical() {
        let a = vec![0.5, 0.5, 0.5];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_is_zero_for_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_returns_zero_for_mismatched_length() {
        assert!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0, 0.0]).abs() < 1e-9);
    }

    #[test]
    fn store_roundtrip_indexes_and_queries() {
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        store
            .index(Role::User, "the meeting with brian is on tuesday")
            .unwrap();
        store
            .index(Role::Assistant, "got it, brian + tuesday noted")
            .unwrap();
        store
            .index(Role::User, "completely unrelated content about weather")
            .unwrap();

        let hits = store.query("when is brian's meeting", 2).unwrap();
        assert_eq!(hits.len(), 2);
        // The brian-related hits should outrank the weather one — but the
        // hash embedder is too crude for stronger ordering claims, so just
        // check that the weather snippet isn't in the top-2.
        for hit in &hits {
            assert!(
                !hit.snippet.contains("weather"),
                "weather should not be in top-2: {hits:?}"
            );
        }
    }

    #[test]
    fn store_skips_empty_snippets() {
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        store.index(Role::User, "").unwrap();
        store.index(Role::User, "   \t\n  ").unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn empty_query_returns_empty_results() {
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        store.index(Role::User, "hello").unwrap();
        assert!(store.query("", 5).unwrap().is_empty());
        assert!(store.query("   ", 5).unwrap().is_empty());
        assert!(store.query("hello", 0).unwrap().is_empty());
    }

    #[test]
    fn fifo_eviction_at_cap() {
        let mut store = RecallStore::open_in_memory(Box::new(ConstEmbedder))
            .expect("open")
            .with_cap(3);
        store.index(Role::User, "first").unwrap();
        store.index(Role::User, "second").unwrap();
        store.index(Role::User, "third").unwrap();
        assert_eq!(store.count().unwrap(), 3);
        store.index(Role::User, "fourth").unwrap();
        assert_eq!(store.count().unwrap(), 3, "cap should hold");

        let hits = store.query("any", 10).unwrap();
        let snippets: Vec<&str> = hits.iter().map(|h| h.snippet.as_str()).collect();
        assert!(!snippets.contains(&"first"), "oldest evicted: {snippets:?}");
        assert!(snippets.contains(&"second"));
        assert!(snippets.contains(&"third"));
        assert!(snippets.contains(&"fourth"));
    }

    #[test]
    fn long_snippet_is_truncated() {
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        let huge = "x".repeat(20_000);
        store.index(Role::User, &huge).unwrap();
        let hits = store.query("xxxx", 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].snippet.len() <= 8 * 1024,
            "snippet should be capped at 8KB, got {}",
            hits[0].snippet.len()
        );
    }

    #[test]
    fn long_multibyte_snippet_does_not_panic_on_cap() {
        // Regression for the byte-boundary slice (issue #26 §C): the 8 KB
        // cap used to slice `&trimmed[..8*1024]` raw, which panics when the
        // boundary lands inside a multibyte glyph. `é` is 2 bytes, so a string
        // of `é` straddles 8192. With `panic="abort"` this aborted the process.
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        let huge = "é".repeat(20_000); // 40 KB, boundary at 8192 splits a glyph
        store.index(Role::User, &huge).unwrap();
        let hits = store.query("é", 1).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.len() <= 8 * 1024);
        assert!(
            hits[0].snippet.is_char_boundary(hits[0].snippet.len()),
            "stored snippet must end on a char boundary"
        );
    }

    #[test]
    fn results_are_sorted_descending_by_score() {
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        for snippet in [
            "the cat sat on the mat",
            "weather forecast for next tuesday",
            "the cat stretched across the rug",
            "currency exchange rates today",
        ] {
            store.index(Role::User, snippet).unwrap();
        }
        let hits = store.query("cat on mat", 4).unwrap();
        // Top hit must be one of the cat lines.
        assert!(
            hits[0].snippet.contains("cat"),
            "top hit should be cat-related: {:?}",
            hits[0]
        );
        // Sorted descending.
        for w in hits.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "results must be descending by score"
            );
        }
    }

    #[test]
    fn query_returns_only_top_k_via_heap() {
        // The rewrite swapped a "score everything, sort, truncate" pass
        // for a running top-k heap. Pin that the heap-based path still
        // returns exactly `k` results in descending order even when the
        // store has many more rows than `k`.
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        for i in 0..20 {
            store
                .index(
                    Role::User,
                    &format!("snippet number {i} talking about cats"),
                )
                .unwrap();
        }
        let hits = store.query("cats", 3).unwrap();
        assert_eq!(hits.len(), 3, "exactly k results");
        for w in hits.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "results must be descending: {:?}",
                hits
            );
        }
    }

    #[test]
    fn query_skips_rows_with_mismatched_dim() {
        // The new query path filters out rows whose vector dimension
        // doesn't match the embedder's current dim (covers the
        // "stored under an older model" case without crashing).
        let mut store = RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        store
            .conn
            .execute(
                "INSERT INTO recall (ts, role, snippet, vec) VALUES ('2026-01-01T00:00:00Z', 'user', 'old-model row', ?1)",
                params![encode_vec(&[1.0_f32, 0.0, 0.0])],
            )
            .expect("seed insert");
        store
            .index(Role::User, "modern cat content matching the embedder dim")
            .unwrap();
        let hits = store.query("cat", 5).unwrap();
        // The old-dim row should be silently skipped, leaving exactly one
        // modern hit.
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("modern cat"));
    }

    #[test]
    fn role_parse_roundtrip() {
        for r in [Role::User, Role::Assistant] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("system"), None);
    }

    #[test]
    fn parse_ollama_embedding_happy_path() {
        let body = json!({ "embedding": [0.1, 0.2, -0.3, 0.0, 1.5] });
        let v = parse_ollama_embedding(&body).expect("parse");
        assert_eq!(v.len(), 5);
        assert!((v[0] - 0.1).abs() < 1e-6);
        assert!((v[2] - -0.3).abs() < 1e-6);
    }

    #[test]
    fn parse_ollama_embedding_rejects_missing_field() {
        let body = json!({ "data": [] });
        let err = parse_ollama_embedding(&body).expect_err("should fail");
        assert!(err.contains("missing 'embedding'"), "got: {err}");
    }

    #[test]
    fn parse_openai_compat_embedding_happy_path() {
        // Real LM Studio shape — extra fields we don't care about included
        // to make sure we tolerate them.
        let body = json!({
            "object": "list",
            "data": [
                {
                    "object": "embedding",
                    "index": 0,
                    "embedding": [0.42, -0.17, 0.99]
                }
            ],
            "model": "nomic-embed-text-v1.5",
            "usage": { "prompt_tokens": 4, "total_tokens": 4 }
        });
        let v = parse_openai_compat_embedding(&body).expect("parse");
        assert_eq!(v.len(), 3);
        assert!((v[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn parse_openai_compat_embedding_rejects_missing_data() {
        let body = json!({ "embedding": [0.1, 0.2] });
        let err = parse_openai_compat_embedding(&body).expect_err("should fail");
        assert!(err.contains("'data[0].embedding'"), "got: {err}");
    }

    #[test]
    fn parse_openai_compat_embedding_rejects_empty_data_array() {
        let body = json!({ "object": "list", "data": [] });
        let err = parse_openai_compat_embedding(&body).expect_err("should fail");
        assert!(err.contains("'data[0].embedding'"), "got: {err}");
    }

    #[test]
    fn json_array_to_f32s_rejects_empty() {
        let err = json_array_to_f32s(&[]).expect_err("should fail");
        assert!(err.contains("empty embedding"), "got: {err}");
    }

    #[test]
    fn json_array_to_f32s_rejects_non_numeric() {
        let arr = vec![json!(0.5), json!("not a number")];
        let err = json_array_to_f32s(&arr).expect_err("should fail");
        assert!(err.contains("non-numeric"), "got: {err}");
    }

    // ────────────────────────────────────────────────────────────────────
    // Live smoke tests — #[ignore]'d so normal `cargo test` skips them.
    // Run on-demand against a real LM Studio with the embed model loaded:
    //
    //   $env:OLLAMA_HOST = "http://localhost:1234"
    //   $env:CLAUDETTE_RECALL_MODEL = "text-embedding-nomic-embed-text-v1.5"
    //   cargo test --lib recall_live -- --ignored --test-threads=1
    //
    // These DO mutate process-wide env vars, so they must run serially —
    // hence `--test-threads=1`. Both tests set the same values, but other
    // tests in the binary may read OLLAMA_HOST and race.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    #[ignore = "requires live LM Studio with embed model loaded"]
    fn recall_live_openai_compat_embed_is_deterministic() {
        let mut e = OpenAICompatEmbedder::new().expect("construct embedder");
        let v1 = e
            .embed("hello from claudette recall smoke")
            .expect("embed 1");
        let v2 = e
            .embed("hello from claudette recall smoke")
            .expect("embed 2");
        assert!(!v1.is_empty(), "got empty vector");
        assert!(
            v1.len() >= 256,
            "expected an embedding ≥256 dims, got {}",
            v1.len()
        );
        assert_eq!(v1.len(), v2.len(), "dim should be stable across calls");
        let cos = cosine_similarity(&v1, &v2);
        assert!(
            (cos - 1.0).abs() < 1e-3,
            "same input should produce ~identical vectors, cos={cos}"
        );
    }

    #[test]
    #[ignore = "requires live LM Studio with embed model loaded"]
    fn recall_live_full_index_query_roundtrip() {
        let embedder: Box<dyn Embedder> =
            Box::new(OpenAICompatEmbedder::new().expect("construct embedder"));
        let mut store = RecallStore::open_in_memory(embedder).expect("open store");

        store
            .index(Role::User, "the meeting with brian is on tuesday at 3pm")
            .unwrap();
        store
            .index(Role::Assistant, "got it — brian, tuesday 3pm noted")
            .unwrap();
        store
            .index(
                Role::User,
                "completely unrelated content about the weather forecast for next week",
            )
            .unwrap();
        store
            .index(
                Role::User,
                "another tangent about currency exchange rates today",
            )
            .unwrap();

        let hits = store.query("when is brian's meeting", 2).expect("query");
        assert_eq!(hits.len(), 2, "asked for top-2: {hits:?}");
        for h in &hits {
            assert!(
                !h.snippet.contains("weather") && !h.snippet.contains("currency"),
                "off-topic snippet leaked into top-2: {h:?}"
            );
        }
        // Top hit must be one of the brian/tuesday lines.
        assert!(
            hits[0].snippet.contains("brian") || hits[0].snippet.contains("tuesday"),
            "top hit should be brian-related, got: {:?}",
            hits[0]
        );
        // Real embedding scores should be meaningfully positive on a
        // semantic match — not just barely-above-zero noise.
        assert!(
            hits[0].score > 0.5,
            "top hit score too low ({}); embedder may be returning noise",
            hits[0].score
        );
    }
}
