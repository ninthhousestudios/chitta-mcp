# Code Review: c06f3aa — RRF hybrid retrieval (dense + FTS + sparse)

Reviewer: Claude Opus 4.6 (cold review, no prior codebase context)
Date: 2026-04-24

---

## CRITICAL

### 1. Sparse weight extraction indexes into flat tensor without accounting for batch dimension

**File:** `src/embedding.rs`, lines 299-314

```rust
let sparse = match outputs.get("sparse_weights") {
    Some(sw) => {
        match sw.try_extract_tensor::<f32>() {
            Ok((_shape, weights)) => {
                let mut map: HashMap<u32, f32> = HashMap::new();
                for (pos, &token_id) in token_ids.iter().enumerate() {
                    let w = weights[pos];
```

The comment says the shape is `[1, seq_len, 1]`, but the code indexes with `weights[pos]` which assumes a flat `[seq_len]` layout. If the tensor is actually `[1, seq_len, 1]`, row-major flattening means index 0 maps correctly, but the extracted `_shape` is discarded without validation. Unlike the dense path (which checks `total != EMBEDDING_DIM`), there is zero shape validation here. If the model ever returns `[batch, seq_len, vocab_size]` (a known BGE-M3 sparse variant), this silently reads garbage values from wrong offsets, producing corrupt sparse embeddings that get persisted to the database and used for ranking.

Fix: validate `_shape` (total elements == `seq_len` or `1 * seq_len * 1`), and reject or adapt if it differs.

### 2. `rrf_candidates` has no lower-bound validation -- zero or negative values cause empty retrieval

**File:** `src/config.rs`, line 82; `src/retrieval.rs`, line 28

```rust
let rrf_candidates: i64 = parse_env_or("CHITTA_RRF_CANDIDATES", 2);
```

```rust
let fetch_limit = k * rrf_candidates;
```

If `CHITTA_RRF_CANDIDATES=0`, then `fetch_limit = 0` and every leg returns zero results, silently breaking search with no error. If negative, the `i64` multiplication wraps to a large negative, which Postgres will reject. There is no validation anywhere (unlike `embedder_pool_size` which has `.max(1)`). This is a production footgun since the variable name does not make "must be >= 1" obvious.

---

## HIGH

### 3. `unwrap_or_default()` on sparse embedding deserialization silently drops corrupt data

**File:** `src/db.rs`, line 465

```rust
let map: HashMap<u32, f32> = serde_json::from_value(json).unwrap_or_default();
```

If a `sparse_embedding` JSONB value is structurally corrupt (e.g., a string where a number is expected, or the schema drifted), this silently returns an empty map instead of signaling an error. That document then gets a sparse dot-product of 0.0, penalizing it in RRF without any indication. This should at least log a warning, or propagate the error so callers know the sparse leg is unreliable.

### 4. `fetch_search_hits_by_ids` sets `similarity = 1.0` for all RRF results -- misleading contract

**File:** `src/db.rs`, lines 480-486

```rust
SELECT id, content, event_time, record_time, tags, source,
       1.0::real AS similarity
FROM memories
```

Every result from the hybrid path gets `similarity = 1.0`, which is then surfaced in the search envelope. Downstream consumers (query logging, agent decision-making) rely on `similarity` to gauge match quality. The query log will record `1.0` for every hybrid hit, poisoning the retrieval research data. The recency re-ranking in `retrieval.rs:109-117` then multiplies `1.0 * (1 + weight * factor)`, which means recency reranking on the hybrid path produces scores > 1.0 -- a range violation that `min_similarity` validation explicitly guards against for inputs.

### 5. `min_similarity` filter is applied only in the dense leg, not enforced post-fusion

**File:** `src/retrieval.rs`, lines 32-33

```rust
let dense_fut = db::search_by_embedding(
    pool, profile, &query_vec, fetch_limit, tags, min_similarity, 0.0, recency_half_life_days,
);
```

The dense leg applies `min_similarity` as a WHERE clause, but FTS and sparse legs have no similarity concept. A document that fails `min_similarity` in the dense leg can still appear in the final results if FTS or sparse scores it highly enough -- the fusion output is never filtered against `min_similarity`. This breaks the documented contract that `min_similarity` is a "cosine-similarity floor."

### 6. Backfill processes rows one-at-a-time with no concurrency

**File:** `src/main.rs`, lines 296-309

```rust
for (id, content) in rows {
    let embed_out = embedder
        .embed_full(&content, "backfill")
        .await
        .context("embedding for backfill")?;
    // ...
    sqlx::query("UPDATE memories SET sparse_embedding = $1 WHERE id = $2")
        .bind(&sparse_json)
        .bind(id)
        .execute(&pool)
        .await
        .context("updating sparse_embedding")?;
}
```

With `embedder_pool_size > 1`, the embedder has multiple ONNX sessions, but the backfill loop serializes everything. For a large database, this is potentially hours slower than necessary. The embed call is already async and pool-aware, so `futures::stream::buffer_unordered` or similar would parallelize trivially. Not a correctness bug but a significant operational concern for any non-trivial database.

### 7. `SPARSE_MISSING_WARN` uses `std::sync::Once` -- first-call-only warning masks ongoing failures

**File:** `src/embedding.rs`, lines 54, 317-321, 328-334

```rust
static SPARSE_MISSING_WARN: Once = Once::new();
// ...
SPARSE_MISSING_WARN.call_once(|| {
    tracing::warn!("sparse_weights output exists but extraction failed: {e}; ...");
});
```

`Once` fires exactly once per process lifetime. If the ONNX model legitimately lacks `sparse_weights` (because the wrong model variant is loaded), this warns once then silently returns empty maps forever. That is fine for a known-missing output. But the extraction-failure branch (line 316) uses the same `Once` -- if the first extraction fails transiently (e.g., OOM), the warning fires once, and all subsequent extractions that fail are completely silent. Two separate failure modes should not share a single `Once` gate.

---

## MEDIUM

### 8. `search_hybrid` clones `query_embed.dense` unnecessarily

**File:** `src/retrieval.rs`, line 29

```rust
let query_vec = Vector::from(query_embed.dense.clone());
```

And in `src/tools/search.rs`, line 113:

```rust
let query_vec = Vector::from(embed_out.dense.clone());
```

When `use_hybrid` is true, the non-hybrid path's `query_vec` is never used, but it is always constructed. The clone in `retrieval.rs` is also unnecessary since `query_embed` is borrowed -- the `Vector::from` could take ownership if the signature were adjusted.

### 9. `ChittaServer::new` has 9 positional parameters -- struct pattern would prevent arg-swap bugs

**File:** `src/mcp.rs`, lines 38-49

```rust
pub fn new(
    pool: PgPool,
    embedder: Arc<Embedder>,
    query_log_enabled: bool,
    recency_weight: f32,
    recency_half_life_days: f32,
    rrf_fts: bool,
    rrf_sparse: bool,
    rrf_k: u32,
    rrf_candidates: i64,
) -> Self {
```

Four booleans and two floats in a row with no type distinction. Swapping `rrf_fts` and `rrf_sparse`, or `recency_weight` and `recency_half_life_days`, compiles silently. The same applies to `search::handle` (10 positional params) and `serve_stdio`. A builder or config struct parameter would eliminate this class of bug.

### 10. FTS query uses `plainto_tsquery` which cannot handle phrase or boolean queries

**File:** `src/db.rs`, lines 428-435

```rust
AND content_tsvector @@ plainto_tsquery('english', $2)
ORDER BY ts_rank(content_tsvector, plainto_tsquery('english', $2)) DESC
```

`plainto_tsquery` strips all operators, so a user query like `"rust AND async"` becomes `'rust' & 'async'` by accident of parsing, not intent. More importantly, `plainto_tsquery` calls `to_tsvector` internally on the query, but the stored column uses `to_tsvector('english', content)`. Both use `'english'`, so this is consistent -- but the choice is hardcoded. Non-English content will get poor FTS recall with no way to configure the dictionary.

### 11. `tsvector` GENERATED column on every INSERT pays cost even when FTS is disabled

**File:** `migrations/0004_fts_and_sparse.sql`, lines 3-4

```sql
ADD COLUMN content_tsvector tsvector
    GENERATED ALWAYS AS (to_tsvector('english', content)) STORED;
```

`STORED` generated columns are computed on every INSERT/UPDATE regardless of whether `CHITTA_RRF_FTS=true`. For large content (up to 8192 tokens), `to_tsvector` is non-trivial. This is a permanent write-path cost for all users, even those who never enable FTS. Consider making FTS opt-in at the migration level or using a trigger-based approach.

### 12. No index on `sparse_embedding IS NULL` for backfill query

**File:** `src/main.rs`, lines 278-285

```sql
SELECT id, content FROM memories
WHERE sparse_embedding IS NULL
ORDER BY id
LIMIT $1
```

This query is called in a loop during backfill. Without a partial index on `(id) WHERE sparse_embedding IS NULL`, it will do a sequential scan of the entire table on each iteration, getting progressively slower as fewer NULLs remain. For a table with millions of rows, this is an O(n*m) backfill where m is the number of batches.

---

## LOW

### 13. `search_by_fts` does not run inside the same transaction as the dense leg

**File:** `src/retrieval.rs`, lines 38-50

The dense leg runs inside a transaction (via `search_by_embedding`'s internal `BEGIN`), but the FTS leg runs against the pool directly. Under concurrent writes, the two legs can see different snapshots -- a newly inserted document might appear in FTS but not dense, or vice versa. For a memory system with low write concurrency this is unlikely to matter, but it is a theoretical consistency gap.

### 14. Sweep script env vars leak between iterations

**File:** `bench/runpod/run-retrieval-sweep.sh`, lines 88-89

```bash
for pair in $ENV_PAIRS; do
    export "$pair"
done
```

Variables set in one config iteration persist into subsequent iterations. If config A sets `CHITTA_RRF_SPARSE=true` and config B does not mention it, B inherits A's value. The `_reset_and_start` function does not clear previous env vars. The current CONFIGS array happens to be ordered so this does not cause problems (dense-only sets only `CHITTA_K`, but `CHITTA_RRF_FTS` and `CHITTA_RRF_SPARSE` default to false when absent). But it is fragile -- reordering configs or adding a config that does not explicitly set all RRF vars will produce wrong results silently.

### 15. `rrf_k: u32` is cast to `f32` which loses precision for large values

**File:** `src/retrieval.rs`, line 55

```rust
let k_const = rrf_k as f32;
```

For `rrf_k > 16_777_216` (2^24), `u32 as f32` loses precision. The default is 60 and reasonable values are all small, so this is not a practical issue, but `rrf_k` has no upper-bound validation.

### 16. `embed_full` is `pub` without documentation

**File:** `src/embedding.rs`, line 169

```rust
pub async fn embed_full(self: &Arc<Self>, text: &str, tool: &'static str) -> Result<EmbedOutput> {
```

The existing `embed` method has a doc comment explaining its contract. `embed_full` has none. `EmbedOutput` also lacks documentation on its fields (what are the semantics of the sparse map keys? token IDs from which vocabulary?).

---

## Testing Gaps

1. **No integration test for the hybrid path.** All existing search tests pass `rrf_fts: false, rrf_sparse: false`. The RRF merge engine is only unit-tested with a helper `rrf_score` function that does not exercise the actual `search_hybrid` function or any database interaction.

2. **No test for backfill correctness.** The backfill command has no test verifying that sparse embeddings are actually written, or that running backfill twice is idempotent (it should be, since the WHERE clause filters NULLs).

3. **No test for FTS query edge cases.** Empty query strings, queries with only stop words (which `plainto_tsquery('english', ...)` reduces to empty), and non-ASCII content are not tested.

4. **No test that `min_similarity` is honored in hybrid mode.** Given finding #5, this test would likely fail.

5. **No test for sparse deserialization failure.** Given finding #3, a corrupt JSONB value silently degrades results.

6. **No test that recency re-ranking in hybrid mode produces valid similarity values.** Given finding #4, a test asserting `similarity <= 1.0` would fail.

---

## Summary

The core RRF fusion logic is algorithmically sound and the code is generally well-structured. The concurrent leg execution with `tokio::join!` and graceful fallbacks on leg failure are good design choices.

The most concerning issues are: (1) the unvalidated sparse tensor shape, which risks persisting corrupt data; (2) the missing validation on `rrf_candidates`, which can silently break search; and (4) the hardcoded `similarity = 1.0` for hydrated RRF results, which poisons query logs and breaks the similarity contract downstream.

The hybrid path has no integration test coverage, meaning all of these issues are currently invisible. The parameter-passing pattern (9+ positional args threaded through 4 layers) is a maintenance hazard that will compound as more config knobs are added.
