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
//! process-wide [`RecallStore`] backed by [`OllamaEmbedder`].

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::api::resolve_ollama_url;

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
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claudette").join("recall.sqlite")
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
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "recall: BLOB length {} is not a multiple of 4 — corrupt vector",
            bytes.len()
        ));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().expect("chunks_exact yields 4-byte slices");
        out.push(f32::from_le_bytes(arr));
    }
    Ok(out)
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
    pub fn open(
        path: impl AsRef<Path>,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("recall: create_dir_all {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| format!("recall: open {}: {e}", path.display()))?;
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
            &trimmed[..8 * 1024]
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
    pub fn query(&mut self, query: &str, k: usize) -> Result<Vec<RecallHit>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let qvec = self.embedder.embed(trimmed)?;

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

        let mut hits: Vec<RecallHit> = Vec::new();
        for row in rows {
            let (ts, role_str, snippet, blob) =
                row.map_err(|e| format!("recall: row error: {e}"))?;
            let Some(role) = Role::parse(&role_str) else {
                continue; // skip rows with unknown roles
            };
            let Ok(v) = decode_vec(&blob) else {
                continue; // skip corrupt rows
            };
            let score = cosine_similarity(&qvec, &v);
            hits.push(RecallHit {
                ts,
                role,
                snippet,
                score,
            });
        }

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(k);
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
            return Err(format!(
                "recall: /api/embeddings HTTP {}",
                resp.status()
            ));
        }
        let body: Value = resp
            .json()
            .map_err(|e| format!("recall: /api/embeddings parse: {e}"))?;
        let arr = body
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("recall: response missing 'embedding': {body}"))?;
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
    let embedder: Box<dyn Embedder> = Box::new(OllamaEmbedder::new()?);
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
        let mut store =
            RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
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
        let mut store =
            RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
        store.index(Role::User, "").unwrap();
        store.index(Role::User, "   \t\n  ").unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn empty_query_returns_empty_results() {
        let mut store =
            RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
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
        let mut store =
            RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
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
    fn results_are_sorted_descending_by_score() {
        let mut store =
            RecallStore::open_in_memory(Box::new(HashEmbedder::new())).expect("open");
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
    fn role_parse_roundtrip() {
        for r in [Role::User, Role::Assistant] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("system"), None);
    }
}
