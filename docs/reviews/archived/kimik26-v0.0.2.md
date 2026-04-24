# Chitta-RS v0.0.2 Code Review Report

**Reviewer:** kimik26  
**Review Date:** 2026-04-21  
**Scope:** research branch (v0.0.2) vs merge-base with main — 197 files changed, ~22,100 lines added  
**Intent:** Implement chitta-rs v0.0.2 with HTTP transport, new memory management tools, pooled embedder with panic recovery, query logging, and comprehensive test coverage

---

## Executive Summary

Overall, the v0.0.2 implementation is well-structured with good separation of concerns, proper error handling following Principle 8, and thoughtful concurrency management. The embedding pool with panic recovery is particularly well-implemented. However, there are several issues ranging from a version mismatch to a potential SQL injection vector that should be addressed before release.

---

## P0 — Critical

| # | File | Issue | Reviewer | Confidence | Route |
|---|------|-------|----------|------------|-------|
| 1 | `Cargo.toml:3` | Version still 0.0.1 in v0.0.2 release | project-standards | 1.00 | `safe_auto -> review-fixer` |
| 2 | `src/main.rs:253` | HTTP CLI args precedence logic inverted — CLI flags ignored when matching defaults | correctness | 0.95 | `safe_auto -> review-fixer` |
| 3 | `src/tools/search.rs:81` | Search query lacks content length limit (no 4MB cap like store) | security | 0.90 | `safe_auto -> review-fixer` |

### Details

#### 1. Version Mismatch in Cargo.toml
- **File:** `Cargo.toml:3`
- **Issue:** Package version still shows 0.0.1 in v0.0.2 release
- **Why it matters:** The package version in Cargo.toml (0.0.1) does not match the advertised v0.0.2 release. This causes version reporting issues via --version flag, breaks semantic versioning expectations for dependencies, and the server info in mcp.rs reports wrong version in instructions.
- **Suggested fix:** Update line 3 to: `version = "0.0.2"`
- **Evidence:** `3 | version = "0.0.1"`

#### 2. HTTP Address/Port Override Logic Bug
- **File:** `src/main.rs:253-258`
- **Issue:** HTTP CLI args precedence logic is inverted
- **Why it matters:** The logic uses CLI args only when they differ from defaults, but the defaults in CLI struct match config defaults exactly. This means CLI --http-addr and --http-port flags are silently ignored when they match the hardcoded defaults, even if config file has different values. Users expect CLI args to always override config.
- **Suggested fix:** Remove the conditional checks. CLI args should always take precedence:
  ```rust
  let http_addr = cli.http_addr;
  let http_port = cli.http_port;
  ```
- **Evidence:**
  ```
  253 |     let http_addr = if cli.http_addr != "127.0.0.1" {
  254 |         cli.http_addr.clone()
  255 |     } else {
  256 |         cfg.http_addr.clone()
  257 |     };
  ```

#### 3. Missing Content Length Limit on Search Query
- **File:** `src/tools/search.rs:81`
- **Issue:** Search query has no byte length limit
- **Why it matters:** While store_memory has 4MB content cap, search query only checks for empty string. A malicious or buggy client could send extremely large queries (GBs) causing memory exhaustion during tokenization or embedding. The embedder will reject >8192 tokens but only after full string is loaded and tokenized.
- **Suggested fix:** Add after line 80: `validate::content_byte_length(TOOL, &query)?;` Or add a separate query-specific limit.
- **Evidence:**
  ```
  81 |     if query.is_empty() {
  82 |         return Err(ChittaError::InvalidArgument {
  90 |     let embedding_vec = embedder.embed(&query, "search_memories").await?;
  ```

---

## P1 — High

| # | File | Issue | Reviewer | Confidence | Route |
|---|------|-------|----------|------------|-------|
| 4 | `src/db.rs:294` | SQL injection pattern — format! used for hnsw.ef_search SET (defense-in-depth) | security | 0.90 | `manual -> review-fixer` |
| 5 | `src/embedding.rs:302` | Session acquire race condition — try_lock without holding semaphore permit | correctness | 0.85 | `manual -> review-fixer` |
| 6 | `src/tools/search.rs:150` | Unbounded query_log tokio::spawn without backpressure | reliability | 0.80 | `gated_auto -> review-fixer` |
| 7 | `src/main.rs:280` | HTTP transport lacks rate limiting and security headers | security | 0.75 | `manual -> downstream-resolver` |
| 8 | `src/embedding.rs:324` | Session replacement doesn't verify new session health | reliability | 0.70 | `manual -> review-fixer` |

### Details

#### 4. SQL Injection in HNSW Configuration
- **File:** `src/db.rs:294`
- **Issue:** Format string SQL injection in hnsw.ef_search SET
- **Why it matters:** While ef_search is computed from validated k parameter, using format!() to build SQL is a dangerous pattern. If the clamp logic is ever bypassed or changed, this becomes an injection vector. The SET LOCAL statement doesn't support bind parameters, but the current approach violates defense-in-depth.
- **Suggested fix:** Add explicit integer validation before format!() and document the security boundary:
  ```rust
  let ef_search: i64 = k.max(1).saturating_mul(4).clamp(HNSW_EF_SEARCH_MIN, HNSW_EF_SEARCH_MAX);
  // SECURITY: ef_search is now guaranteed to be a valid integer in [200, 1000]
  // This format! is safe because we've validated the range.
  sqlx::query(&format!("set local hnsw.ef_search = {ef_search}"))
  ```
- **Evidence:**
  ```
  274 |     let ef_search = (k.max(1) * 4).clamp(HNSW_EF_SEARCH_MIN, HNSW_EF_SEARCH_MAX);
  294 |     sqlx::query(&format!("set local hnsw.ef_search = {ef_search}"))
  ```

#### 5. Race Condition in Session Acquisition
- **File:** `src/embedding.rs:302`
- **Issue:** Session acquire race condition in embedder pool
- **Why it matters:** acquire_session() does round-robin try_lock without holding the semaphore permit. Between acquiring the permit (line 194) and finding a session (line 199), another thread could lock the session we selected. The fallback to index 0 when all try_locks fail is also racy and could cause contention on session 0.
- **Suggested fix:** Restructure to acquire session index while holding permit, or use a fairer allocation strategy. One approach: maintain an AtomicUsize counter for round-robin selection, then try_lock that specific session. If locked, retry with next index up to pool_size attempts.
- **Evidence:**
  ```
  194 |         let _permit = self.semaphore.acquire().await.map_err(|_| {
  199 |         let session_idx = self.acquire_session();
  302 |     fn acquire_session(&self) -> usize {
  303 |         for i in 0..self.sessions.len() {
  304 |             if self.sessions[i].try_lock().is_ok() {
  305 |                 return i;
  306 |             }
  307 |         }
  310 |         0
  ```

#### 6. Query Log Fire-and-Forget Without Backpressure
- **File:** `src/tools/search.rs:150`
- **Issue:** Unbounded query_log tasks can exhaust resources
- **Why it matters:** The tokio::spawn for query_log insertion has no timeout, no backpressure, and no limit on concurrent tasks. Under heavy search load, this could spawn unlimited tasks causing memory exhaustion. Failed inserts are only logged via tracing::warn, potentially creating log spam.
- **Suggested fix:** Add timeout and consider using a bounded channel or semaphore for query_log writes:
  ```rust
  tokio::spawn(async move {
      let timeout = tokio::time::Duration::from_secs(5);
      if let Err(e) = tokio::time::timeout(timeout, db::insert_query_log(...)).await {
          tracing::warn!("query_log insert timed out or failed: {e}");
      }
  });
  ```
- **Evidence:**
  ```
  150 |         tokio::spawn(async move {
  151 |             if let Err(e) = db::insert_query_log(
  167 |                 tracing::warn!("query log insert failed: {e}");
  ```

#### 7. Missing HTTP Security Headers and Rate Limiting
- **File:** `src/main.rs:280`
- **Issue:** HTTP transport lacks rate limiting and security headers
- **Why it matters:** The HTTP MCP endpoint has bearer auth but no rate limiting, making it vulnerable to brute force token attacks and DoS via request flooding. Also missing security headers (CSP, HSTS, etc.) that would be expected in production deployments.
- **Suggested fix:** Add tower-http layers for rate limiting (e.g., per-IP or per-token) and security headers. Document the need for reverse proxy (nginx/traefik) in production for additional protection.
- **Evidence:**
  ```
  280 |     #[allow(deprecated)]
  281 |     let app = axum::Router::new()
  282 |         .route("/mcp", any_service(mcp_service))
  283 |         .layer(ValidateRequestHeaderLayer::bearer(&bearer_token));
  ```

#### 8. Potential Deadlock in Session Replacement
- **File:** `src/embedding.rs:324`
- **Issue:** Session replacement could deadlock on poisoned mutex
- **Why it matters:** replace_session() uses lock().unwrap_or_else(|e| e.into_inner()) which handles poisoned mutexes. However, if the session thread panicked while holding the lock, the poisoned data may be corrupted. The current code tries to replace it but doesn't verify the new session works.
- **Suggested fix:** After session replacement, perform a lightweight health check (dummy inference) before marking the slot as healthy. If replacement fails, consider removing the slot from the pool entirely rather than leaving it degraded.
- **Evidence:**
  ```
  324 |                 *self.sessions[idx].lock().unwrap_or_else(|e| e.into_inner()) = new_session;
  328 |             Err(e) => {
  329 |                 tracing::error!(
  330 |                     session = idx,
  331 |                     error = %e,
  332 |                     "failed to create replacement session — slot degraded"
  ```

---

## P2 — Moderate

| # | File | Issue | Reviewer | Confidence | Route |
|---|------|-------|----------|------------|-------|
| 9 | `migrations/0002_query_log.sql` | query_log table grows unbounded with no retention policy | maintainability | 0.85 | `manual -> downstream-resolver` |
| 10 | `src/main.rs:165` | Replay subcommand doesn't verify embedding consistency on model drift | correctness | 0.75 | `manual -> downstream-resolver` |
| 11 | `src/tools/store.rs:77` | Redundant idempotency lookup creates TOCTOU race (extra DB round-trip) | correctness | 0.90 | `safe_auto -> review-fixer` |
| 12 | `src/tools/validate.rs:12` | Profile names case-sensitive without documented behavior | api-contract | 0.70 | `advisory -> human` |
| 13 | `src/envelope.rs:28` | Token budget estimation uses approximation without validation | api-contract | 0.80 | `advisory -> human` |

### Details

#### 9. Missing Query Log Cleanup
- **File:** `migrations/0002_query_log.sql`
- **Issue:** query_log table grows unbounded with no retention policy
- **Why it matters:** The query_log table is append-only with no TTL, retention limit, or archival process. In production with high search volume, this could consume unlimited disk space. No migration or tool provides cleanup functionality.
- **Suggested fix:** Add a retention policy and scheduled cleanup:
  1. Add CHITTA_QUERY_LOG_RETENTION_DAYS config (default 30)
  2. Add periodic cleanup task or document cron job using DELETE FROM query_log WHERE created_at < NOW() - INTERVAL '30 days'
- **Evidence:**
  ```
  372 | /// Append-only insert into `query_log`. Fire-and-forget from the search
  373 | /// handler — errors are logged but never propagated to the caller.
  ```

#### 10. Replay Subcommand Doesn't Verify Embedding Consistency
- **File:** `src/main.rs:165`
- **Issue:** Replay subcommand may report false positives on model drift
- **Why it matters:** The replay command re-runs searches using stored embeddings but doesn't verify the current model produces the same embedding for the same query text. If the model was updated or changed, the replay comparison is comparing apples to oranges but reports it as regression.
- **Suggested fix:** Optionally re-embed the query text and compare with stored embedding to detect model drift. If embeddings differ significantly, warn that replay results may not be comparable.
- **Evidence:**
  ```
  165 |         let (new_hits, _total) = db::search_by_embedding(
  166 |             &pool,
  167 |             &entry.profile,
  168 |             &entry.embedding,  // Uses stored embedding, not recomputed
  169 |             entry.k as i64,
  170 |             &entry.tags,
  171 |             entry.min_similarity,
  172 |         )
  ```

#### 11. Idempotency Key Lookup Race Condition
- **File:** `src/tools/store.rs:77`
- **Issue:** Pre-insert idempotency check creates TOCTOU race
- **Why it matters:** The pre-flight SELECT for idempotency key (lines 77-81) followed by INSERT creates a time-of-check-to-time-of-use race. The database's unique constraint catches this, but the code makes two round trips unnecessarily. The INSERT-or-fetch logic in db.rs handles this correctly, but store.rs does an extra lookup first.
- **Suggested fix:** Remove the pre-flight SELECT (lines 77-81). The db::insert_or_fetch_memory already handles the conflict case efficiently with a single round-trip via ON CONFLICT or the unique violation handler.
- **Evidence:**
  ```
  77 |     if let Some(existing) =
  78 |         db::find_by_idempotency_key(pool, &args.profile, &args.idempotency_key).await?
  79 |     {
  80 |         return Ok(row_to_output(existing, true));
  81 |     }
  ```

#### 12. Profile Names Not Normalized
- **File:** `src/tools/validate.rs:12`
- **Issue:** Profile names not case-normalized leading to isolation issues
- **Why it matters:** Profile 'MyProfile' and 'myprofile' are treated as different profiles due to case sensitivity. This is technically correct for isolation but may confuse users expecting case-insensitive behavior. No documentation clarifies this behavior.
- **Suggested fix:** Document the case-sensitive behavior in API docs and tool descriptions. Consider adding a note in error messages when a profile isn't found suggesting to check case.
- **Evidence:**
  ```
  12 | pub fn profile(tool: &'static str, value: &str) -> Result<()> {
  16 |         value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
  ```

#### 13. Token Budget Estimation Not Validated Against Actual
- **File:** `src/envelope.rs:28`
- **Issue:** Token budget estimation uses approximation without validation
- **Why it matters:** The estimate_tokens function uses ceil(bytes/4) which is a rough approximation. Actual token count from a real tokenizer could differ significantly (e.g., unicode, special characters). This may cause budget to be exceeded or underutilized.
- **Suggested fix:** Document the approximation in API docs. Consider adding a flag to use real tokenizer when accuracy matters more than performance, or tighten the estimation heuristic.
- **Evidence:**
  ```
  24 | /// Approximate token estimator: `ceil(bytes / 4)`.
  25 | ///
  26 | /// Documented as approximate in `docs/starting-shape.md`. Tightens when we
  27 | /// put a real tokenizer on the hot path.
  28 | pub fn estimate_tokens<T: Serialize>(payload: &T) -> u64 {
  ```

---

## P3 — Low

| # | File | Issue | Reviewer | Confidence | Route |
|---|------|-------|----------|------------|-------|
| 14 | `src/tools/list.rs:57` | Tracing span missing tags filter field | maintainability | 0.90 | `safe_auto -> review-fixer` |
| 15 | `tests/integration.rs:57` | Tests share embedder state without documented isolation rationale | testing | 0.60 | `advisory -> review-fixer` |
| 16 | `src/db.rs:92` | Error message could leak internal details | maintainability | 0.70 | `safe_auto -> review-fixer` |
| 17 | `src/main.rs:282` | HTTP endpoint doesn't validate Content-Type header | security | 0.60 | `gated_auto -> review-fixer` |
| 18 | `src/tools/search.rs:140` | Query log latency measurement doesn't capture full end-to-end time | maintainability | 0.80 | `advisory -> review-fixer` |

### Details

#### 14. Tracing Instrumentation Missing Fields
- **File:** `src/tools/list.rs:57`
- **Issue:** Some tracing spans don't capture important fields
- **Why it matters:** The list_recent_memories tracing span doesn't capture the tags filter which is important for debugging. Delete and get handlers also miss some useful fields.
- **Suggested fix:** Add tags field to tracing::instrument:
  ```rust
  #[tracing::instrument(
      name = "tool.list_recent_memories",
      skip(pool, args),
      fields(profile = %args.profile, limit = ?args.limit, tags = ?args.tags),
  )]
  ```
- **Evidence:**
  ```
  57 | #[tracing::instrument(
  58 |     name = "tool.list_recent_memories",
  59 |     skip(pool, args),
  60 |     fields(profile = %args.profile, limit = ?args.limit),
  61 | )]
  ```

#### 15. Tests Use Shared Embedder Without Isolation
- **File:** `tests/integration.rs:57`
- **Issue:** Integration tests share embedder state but not DB state
- **Why it matters:** While tests create fresh DB pools per test, they share the embedder via OnceCell. If a test were to corrupt the embedder state (unlikely with current read-only usage), it would affect other tests. Also limits parallel test execution.
- **Suggested fix:** Document this design decision. The current approach is valid since embedder is effectively immutable after load, but add a comment explaining why shared embedder is safe (read-only after initialization).
- **Evidence:**
  ```
  40 | // Embedder load (~1-2s ONNX startup) is shared via a static because it's a
  41 | // pure-sync resource safe to reuse across tests.
  57 | static SHARED: OnceCell<Option<SharedSetup>> = OnceCell::const_new();
  ```

#### 16. Error Messages Could Leak Internal Details
- **File:** `src/db.rs:92`
- **Issue:** Some error messages include internal details
- **Why it matters:** The unique violation recovery error at line 92-96 could expose internal database details if the row disappears between the conflict detection and lookup (extremely unlikely but possible).
- **Suggested fix:** Simplify the error message to not suggest internal state:
  ```rust
  .ok_or_else(|| {
      ChittaError::Internal(
          "idempotency conflict resolution failed".to_string(),
      )
  })?;
  ```
- **Evidence:**
  ```
  92 |                     .ok_or_else(|| {
  93 |                         ChittaError::Internal(
  94 |                             "unique violation without recoverable row".to_string(),
  95 |                         )
  96 |                     })?;
  ```

#### 17. Missing Content-Type Validation in HTTP Mode
- **File:** `src/main.rs:282`
- **Issue:** HTTP endpoint doesn't validate Content-Type header
- **Why it matters:** The MCP HTTP endpoint accepts any Content-Type. While rmcp handles framing, strict Content-Type validation (application/json) would catch misconfigured clients earlier.
- **Suggested fix:** Add a layer to validate Content-Type is application/json for POST requests, or document that the endpoint accepts any encoding that rmcp can parse.
- **Evidence:**
  ```
  281 |     let app = axum::Router::new()
  282 |         .route("/mcp", any_service(mcp_service))
  283 |         .layer(ValidateRequestHeaderLayer::bearer(&bearer_token));
  ```

#### 18. Query Log Latency Measurement Incomplete
- **File:** `src/tools/search.rs:140`
- **Issue:** Query log latency doesn't capture full search time
- **Why it matters:** The latency_ms captures time from search start to envelope creation, but doesn't include the time to spawn the log task or any queuing delays before the log insert actually executes.
- **Suggested fix:** Rename to 'processing_ms' or document that this is 'time to generate results' not including logging overhead. Consider adding a separate 'total_latency' metric if end-to-end timing is needed.
- **Evidence:**
  ```
  75 |     let search_start = std::time::Instant::now();
  140 |         let latency_ms = search_start.elapsed().as_millis() as i64;
  141 |         let result_ids: Vec<Uuid> = envelope.results.iter().map(|h| h.id).collect();
  ```

---

## Positive Findings (Well-Implemented Areas)

1. **Excellent Error Contract**: The Principle 8 error contract (`tool`, `constraint`, `next_action`) is consistently implemented across all error paths with comprehensive test coverage in `tests/contract.rs`.

2. **Panic Recovery**: The embedding pool's use of `catch_unwind` with session replacement is a robust approach for handling ONNX runtime instability.

3. **Concurrency Safety**: The semaphore + mutex approach for session pooling correctly handles the `!Send` nature of ONNX sessions.

4. **Input Validation**: Comprehensive validators in `src/tools/validate.rs` cover all major attack vectors (content length, token count, profile format, etc.).

5. **Test Coverage**: Good separation of L0 (contract) and L2 (integration) tests with clean skip logic for missing dependencies.

---

## API Contract Changes (Breaking)

The Rust v0.0.2 implementation intentionally diverges from Python chitta per `docs/contract-alignment.md`:

- **store_memory**: `profile` and `idempotency_key` now required; `source`, `metadata`, `auto_link` removed; response shape changed
- **get_memory**: `memory_id` renamed to `id`; `profile` now required; `graph_depth`, `reinforce`, `include_invalidated` removed  
- **search_memories**: `limit` renamed to `k`; `budget_spent` renamed to `budget_spent_tokens`; `refinement_advice` removed; `min_similarity` added

These are **documented intentional breaking changes** for the Rust rewrite.

---

## Testing Gaps

- No test for HTTP mode authentication failure paths
- No test for embedding panic recovery and session replacement
- No test for concurrent session pool exhaustion scenarios  
- No test for query_log database failure handling
- No negative test for SQL injection attempts on ef_search validation

---

## Recommendations Summary

### Must Fix Before Release (P0-P1):
1. Update Cargo.toml version to 0.0.2
2. Fix HTTP CLI args precedence logic
3. Add content length limit to search query
4. Add integer validation comment before SQL format!()
5. Fix session acquisition race condition
6. Add query_log backpressure protection

### Should Fix (P2):
1. Add query_log retention policy
2. Remove redundant idempotency lookup in store.rs
3. Document profile case-sensitivity behavior

### Nice to Have (P3):
1. Enhance tracing fields
2. Add Content-Type validation
3. Clarify latency measurement scope

---

## Verdict

> **Verdict:** Ready with fixes
>
> **Reasoning:** Three critical issues must be fixed before release: version mismatch (P0), HTTP CLI args logic bug (P0), and missing search query length limit (P0). The SQL format pattern (P1) should be hardened with explicit validation. Session pool race condition (P1) should be restructured for correctness under concurrent load.
>
> **Fix order:** P0 version → P0 CLI args → P0 search validation → P1 SQL validation → P1 session race → P1 query_log backpressure

**Action required:** Apply the three P0 fixes and verify with `cargo test`. The P1 fixes can follow in a patch release if needed for timeline.
