# Code Review: c06f3aafd0cfddeea064a856b4c2c21b47d93957

**Feature:** RRF hybrid retrieval (dense + FTS + sparse)
**Author:** josh, Co-Authored-By: Claude Opus 4.6
**Reviewer:** glm-5.1 (opencode)
**Date:** 2026-04-24

---

## Summary

This commit adds Reciprocal Rank Fusion (RRF) hybrid retrieval combining three legs: dense (HNSW, always on), full-text search (tsvector + GIN), and sparse lexical re-ranking (BGE-M3 `sparse_weights`). Each leg is independently toggled via env vars. When both FTS and sparse are disabled (the default), behavior is identical to previous versions.

The commit touches 14 files (+655 / -36 lines), adding a migration, a new `retrieval.rs` module, extending `embedding.rs` and `db.rs` with sparse/FTS support, a backfill subcommand, and a benchmarking sweep script.

---

## Migration (`0004_fts_and_sparse.sql`)

### Positive

- Clean, minimal migration. The `GENERATED ALWAYS AS` tsvector column is the right approach — it stays in sync with `content` automatically and requires no trigger maintenance.
- GIN index on `content_tsvector` is the correct index type for tsquery.
- `sparse_embedding` as nullable JSONB with `DEFAULT NULL` is safe for existing rows.

### Issues

1. **[Medium] No `DROP` counterpart / rollback migration.** If 0004 needs to be rolled back, the columns and index must be dropped manually. Consider adding a down-migration or documenting the rollback steps.

2. **[Low] Missing `CONCURRENTLY` on GIN index.** `CREATE INDEX` (without `CONCURRENTLY`) takes a full table lock, which blocks writes on large tables. For production databases with millions of rows, this could cause downtime. In practice, if the migration runs before significant data is inserted, this is fine. Worth a comment noting this risk.

3. **[Low] No `COMMENT` on the new columns.** The existing codebase doesn't have column comments either, so this is consistent, but the `sparse_embedding` JSONB schema (expected key/value shapes) is implicit. Consider constraining with a CHECK or at minimum documenting the expected schema shape.

---

## Config (`src/config.rs`)

### Positive

- Clean use of `parse_env_or` for all new fields. Defaults are backward-compatible (FTS=false, sparse=false).
- The `rrf_sparse && !rrf_fts` warning is a nice operational touch.

### Issues

4. **[High] `rrf_candidates: i64` default of 2 means the multiplier for fetch is `k * 2`.** With `k=10` (default), `fetch_limit=20`. This is very low for a fusion algorithm — RRF works best when each leg retrieves significantly more candidates than the final k. The [original RRF paper](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf) doesn't specify this parameter, but common practice uses 3-10x k. A default of 2 means dense+FTS each return only 20 candidates for a top-10 output, which may not provide enough overlap for fusion to add value. Consider raising the default to at least `k * 5` or documenting why 2 is chosen.

5. **[Medium] `rrf_k: u32` with default 60 is fine but undocumented range.** The RRF constant `k` should be > 0 to avoid division issues. A value of 0 would make `1/(0+0)` = infinity for rank 0. There's no validation that `rrf_k > 0`. Add a `.max(1)` floor or an explicit validation check in `Config::from_env`.

6. **[Low] No validation for `sparse_threshold` range.** Values < 0 would match nothing; values > 1.0 (for BGE-M3 weights) would match everything. A `.clamp(0.0, 1.0)` or at least a warning would be defensive.

7. **[Low] `rrf_candidates` semantically represents a multiplier, not a count.** The name suggests "number of candidates" but it's used as `k * rrf_candidates`. Consider renaming to `rrf_fetch_multiplier` or similar to make intent clear.

---

## Embedding (`src/embedding.rs`)

### Positive

- `EmbedOutput` struct is clean. `embed()` now delegates to `embed_full()` — no duplication.
- `Once` for the sparse-missing warning is good; avoids log spam.
- The sparse extraction maps token IDs to their max weight across positions, which is the correct BGE-M3 sparse representation.

### Issues

8. **[Medium] `SPARSE_MISSING_WARN` uses `Once` which fires only once globally.** If the ONNX model is loaded in a test that doesn't have `sparse_weights`, the warning fires and then never fires again in subsequent embed calls in the same process. This is likely fine for production but could mask issues in long-running processes where the model output changes. `Once` is the standard pattern here though, so this is acceptable.

9. **[Medium] Duplicate token handling in sparse extraction.** The code iterates `token_ids` by position and keeps the max weight per token. This is correct for BGE-M3 (where the same token can appear at multiple positions with different weights). However, the current code uses `map.entry(token_id).or_insert(0.0)` followed by a max-check, which could be simplified to `map.entry(token_id).and_modify(|v| *v = v.max(w)).or_insert(w)`. The current `.or_insert(0.0)` + conditional assignment works but is subtly harder to reason about than a single entry API call.

10. **[Low] `token_ids` is collected as `Vec<u32>` from `ids` (`Vec<u32>`) only to iterate positionally alongside the sparse_weights tensor.** This is an extra allocation that could be avoided by just referencing `ids` directly, since `ids` is already `Vec<u32>`. The clone happens on every `embed_full()` call.

---

## Retrieval Engine (`src/retrieval.rs`)

### Positive

- Clean RRF implementation. The algorithm is correct: each leg contributes `1/(k+rank)` per document.
- Unit tests for RRF formula are solid.
- Proper error handling: FTS and sparse leg failures are logged and degraded gracefully rather than failing the whole search.
- Recency re-ranking is correctly applied post-fusion, not pre-fusion.

### Issues

11. **[High] Dense leg runs with `recency_weight=0.0` but the original `min_similarity` filter is still applied.** When hybrid mode is on, `min_similarity` is a cosine threshold that filters candidates *before* FTS/sparse can contribute. This means documents that FTS would strongly rank could be excluded because their cosine similarity is below the threshold. The `min_similarity` filter should arguably only apply when not in hybrid mode, or be documented as intentionally conservative. At minimum, when hybrid is active, consider setting `min_similarity` to 0.0 or a much lower value for the dense leg to ensure FTS candidates can be surfaced.

12. **[High] `search_hybrid` has 12 parameters.** This is a code smell (long parameter list). Consider grouping the RRF-related parameters into a struct (`RrfConfig` or `HybridConfig`) that can be passed as a single argument. This would also make it easier to pass these through the call chain (search handler → ChittaServer → retrieval).

13. **[Medium] The `total` count returned by `search_by_embedding` for the dense leg may be misleading in hybrid mode.** The `total` comes from the dense ANN search's `COUNT(*) OVER()`, which reflects the number of results passing the cosine filter — not the total number of documents in the profile. In hybrid mode, this count doesn't account for FTS-only matches. This could confuse callers comparing `total_available` with `len(results)`.

14. **[Medium] Sparse re-ranking only considers candidates already in `rrf_scores`.** This means if FTS is disabled and dense returns only documents above `min_similarity`, the sparse leg operates on a very narrow candidate set. This may be intentional (sparse as a re-ranker, not a retreiver) but it limits sparse's effectiveness when FTS is off.

15. **[Low] `fetch_sparse_embeddings` uses `serde_json::from_value` with `unwrap_or_default()`.** If a sparse_embedding JSONB value is malformed (e.g., wrong types, corrupted), it silently becomes an empty HashMap, effectively removing that document from sparse ranking. This is safe but invisible — consider logging a warning on deserialization failure.

16. **[Low] No `#[instrument]` on `search_hybrid`.** The rest of the tool handlers use `tracing::instrument`. Adding instrumentation here would help with observability in production.

---

## DB Layer (`src/db.rs`)

### Positive

- All existing `SELECT` and `INSERT` queries are correctly extended with the new `sparse_embedding` column.
- New functions (`search_by_fts`, `fetch_sparse_embeddings`, `fetch_search_hits_by_ids`) are well-structured.
- `fetch_search_hits_by_ids` correctly re-orders results to match the RRF rank order using a HashMap.

### Issues

17. **[High] `search_by_fts` uses `plainto_tsquery` which strips operators.** This means complex queries like `"postgres & pool"` are treated as literal text, not as Boolean tsquery expressions. This is the safe default, but users expecting Boolean search semantics will be disappointed. Worth documenting the intentional limitation.

18. **[Medium] `search_by_fts` does not support `min_similarity`.** The dense leg applies a cosine floor, but FTS returns results ranked purely by `ts_rank`. There's no way to filter low-quality FTS matches. For short queries, `plainto_tsquery` can match very common words and return many low-relevance results.

19. **[Medium] `fetch_search_hits_by_ids` uses `1.0::real AS similarity` as a placeholder.** In hybrid mode, the final RRF score is lost — all returned hits have `similarity = 1.0` when fetched from the DB, and then the recency re-ranking is applied on top. If recency rewriting isn't active, all results report `similarity = 1.0` to the caller, which misrepresents their actual relevance. The RRF score should ideally be propagated into the `SearchHit.similarity` field.

20. **[Low] `search_by_fts` and `fetch_sparse_embeddings` don't respect profile isolation if the `tags` filter is empty.** The `tags` filter uses `($4::text[] = '{}' OR tags && $4)` which correctly passes `{}` as "no filter" — this is fine, worth confirming the empty-array handling matches the dense leg.

---

## Backfill Subcommand (`src/main.rs`)

### Positive

- Clean batch loop pattern with progress logging.
- Correctly uses `embed_full` to compute both dense and sparse.

### Issues

21. **[High] Backfill only writes `sparse_embedding` but doesn't update `content_tsvector`.** The tsvector column is `GENERATED ALWAYS AS`, so PostgreSQL maintains it automatically on INSERT/UPDATE. However, the backfill only updates `sparse_embedding`, which means the tsvector column was already populated by the migration's `GENERATED` expression for existing rows. This is actually fine — PostgreSQL will auto-generate the tsvector on the migration ALTER. But it's worth verifying with `\d+ memories` that the generated column is indeed populated for all existing rows after migration. (It should be, since `STORED` generated columns are populated immediately on `ALTER TABLE ADD COLUMN`.)

22. **[Medium] Sequential processing within a batch.** Each row in `rows` is embedded and updated one at a time. For large backfills, this could be significantly faster by parallelizing the embedding calls (the `Embedder` already has a semaphore-backed pool). Consider `futures::stream::iter(rows).map_concurrent(...)` or batching embed calls.

23. **[Medium] No `--profile` filter.** The backfill processes all profiles' rows, which may not be desired if only some profiles need sparse embeddings. Consider adding an optional profile filter.

24. **[Low] `batch_size` default of 100 and the termination condition `batch_len < batch_size` could miss rows if rows are deleted between batches.** The `ORDER BY id` pagination is correct for insert-only workloads but could skip or loop if rows are inserted with lower UUIDs (unlikely with v7 UUIDs) or deleted.

25. **[Low] The backfill command doesn't print anything on success until the final line, and uses `tracing::info` for batch progress.** The `println!` at the end is the only stdout output. Consider adding a `--quiet` flag or progress bar.

---

## Tool Handlers (`src/tools/`)

### `store.rs`

26. **[Low] `sparse_embedding` is always stored as `Some(sparse_json)`, never `None`.** This means every row in the table will have a non-null `sparse_embedding`. This is fine but makes the nullable column effectively NOT NULL. If the ONNX model doesn't produce `sparse_weights`, the field will be an empty JSON object `{}` (from `HashMap::new()`). Consider whether it should be `None` in that case instead, to save storage and make `IS NOT NULL` queries meaningful.

### `update.rs`

27. **[Low] When `content` is not updated, `sparse_embedding` stays as-is.** This is correct behavior — only re-embed if content changes. But if the sparse model is improved later (weights change), there's no way to force a re-embed without changing content. This is an edge case that the backfill subcommand addresses.

### `search.rs`

28. **[Medium] `use_hybrid = rrf_fts || rrf_sparse` means even when only `rrf_sparse=true`, the hybrid path is taken.** This is intentional (sparse needs dense candidates to operate on), but the naming `use_hybrid` is slightly misleading. When only sparse is on, the code calls `search_hybrid` which only runs the dense leg (since `rrf_fts=false`), then applies sparse re-ranking. This is functionally correct but could confuse a reader. Consider a comment.

### `health.rs`

29. **[Low] `retrieval_legs` is `Vec<&'static str>` assembled at health-check time.** This is fine. Dense is always included, which is correct.

---

## MCP / Server (`src/mcp.rs`, `src/main.rs`)

### Issues

30. **[Medium] Parameter sprawl through `ChittaServer::new`.** The `new` function now takes 9 parameters (pool, embedder, query_log, rrf_fts, rrf_sparse, rrf_k, rrf_candidates, recency_weight, recency_half_life_days). Combined with the same parameters threaded through `handle_search` and the HTTP handler, this is ripe for a config struct. The `Config` type already exists but is consumed at startup. Consider passing an `Arc<RrfConfig>` or `Arc<SearchConfig>` that bundles these.

31. **[Low] HTTP handler clones the entire `Config`-derived parameter set into each request's `ChittaServer`.** This is cheap (just booleans and numbers) but architecturally, a shared `Arc<SearchConfig>` would be cleaner.

---

## Integration Tests (`tests/integration.rs`)

### Issues

32. **[Medium] No tests for hybrid/RRF search behavior.** The existing tests all pass `rrf_fts=false, rrf_sparse=false` (the backward-compatible default). There are no tests exercising the FTS leg, sparse leg, or the fusion logic end-to-end. Given the complexity of the RRF merge, this is a significant gap. At minimum, add tests that:
    - Verify FTS search returns expected IDs for exact-text matches.
    - Verify `search_hybrid` with FTS enabled returns results that dense-only would miss.
    - Verify the empty-result path when no legs produce candidates above `min_similarity`.

33. **[Low] Test harnesses now have 5 additional parameters in each `Config` construction.** This is boilerplate that a `Config::test_default()` method could eliminate.

---

## Benchmark Script (`bench/runpod/run-retrieval-sweep.sh`)

### Positive

- Clean, well-structured sweep script. Each config resets the database and restarts the server.

### Issues

34. **[Medium] Environment variable leakage between configs.** The `for pair in $ENV_PAIRS; do export "$pair"; done` pattern persists vars across iterations. If config A sets `CHITTA_RRF_FTS=true` and config B doesn't include it, config B will still have `CHITTA_RRF_FTS=true` from the previous iteration. Fix: `unset` all relevant vars at the start of each iteration, or use `env -i` to start clean.

35. **[Low] `_reset_and_start` drops and recreates the database as the `postgres` superuser but creates it with `OWNER chitta`.** If the `chitta` user doesn't exist, this will fail. The script assumes a specific DB setup.

36. **[Low] No error handling if `chitta-rs` fails to start.** The `sleep 5` after starting the server is a fixed wait with no health-check verification.

---

## Overall Assessment

### Strengths

- The backward-compatibility story is excellent: defaults preserve identical behavior to v0.0.2.
- Graceful degradation: FTS and sparse legs log warnings and fall back rather than failing the entire search.
- Clean separation: `retrieval.rs` is a focused module with unit-testable core logic.
- The migration is minimal and safe for existing data.

### Critical Issues to Address

1. **`min_similarity` interaction with hybrid mode** (issue #11): When hybrid is on, the dense leg still applies the cosine threshold, which can exclude FTS-strong candidates before fusion. This may silently reduce the effectiveness of hybrid retrieval.

2. **`rrf_candidates` default of 2** (issue #4): This is too low for meaningful fusion. Consider `5` or higher as the default, or at least document the trade-off.

3. **`rrf_k` validation** (issue #5): A `CHITTA_RRF_K=0` would cause `1/(0+0)` = infinity for rank 0 documents, producing `inf` scores. Add a `.max(1)` floor.

4. **Missing RRF score propagation** (issue #19): All hybrid-mode results report `similarity=1.0` to the caller. This makes the LLM agent's ability to assess result quality significantly worse.

5. **No integration tests for hybrid retrieval** (issue #32): The core new functionality has no end-to-end test coverage.

### Recommended Priorities

| Priority | Issue | Description |
|----------|-------|-------------|
| P0 | #5 | `rrf_k=0` causes inf scores |
| P0 | #19 | RRF scores lost, all results report similarity=1.0 |
| P1 | #11 | min_similarity filters out FTS candidates in hybrid mode |
| P1 | #4 | rrf_candidates default too low for meaningful fusion |
| P1 | #32 | No integration tests for hybrid search |
| P2 | #12, #30 | Parameter sprawl — introduce RrfConfig struct |
| P2 | #34 | Env var leakage in sweep script |
| P2 | #22 | Sequential backfill is slow |
| P3 | #17 | FTS limited to plainto_tsquery |
| P3 | #26 | sparse_embedding stored as {} not NULL for empty |
| P3 | #15 | Silent deserialization failures in fetch_sparse_embeddings |