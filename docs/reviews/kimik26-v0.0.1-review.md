# chitta-rs v0.0.1 — Code Review (kimi-k2.6)

**Review date:** 2026-04-21  
**Scope:** `rust/` directory — 17 source/test files, ~2,500 lines of Rust  
**Toolchain:** Rust 2024 edition (1.85+), `cargo check` and `cargo clippy -D warnings` both clean  
**Tests exercised:** `cargo test --lib` (28 passed), `cargo test --test contract` (10 passed)  

---

## Executive Summary

chitta-rs v0.0.1 is a tightly-scoped, well-reasoned rewrite of the Python chitta memory server. The codebase exhibits mature engineering discipline: every dependency is justified in a comment, error messages are actionable by design, and the three-tool surface is minimal enough to audit in an afternoon. The code compiles without warnings, clippy is clean, and the contract-test suite provides strong regression protection for the wire format.

The primary risks are not in the code that is here, but in the code that is *not yet* here: the global `Mutex<Session>` around the ONNX runtime, the `format!`-based SQL `SET LOCAL`, and the lack of timeout guards around `spawn_blocking` tasks are all acceptable for the stdio-only v0.0.1 but become correctness or security issues the moment HTTP transport or concurrent clients arrive. The review flags these as "v0.0.2 blockers" rather than immediate bugs.

**Overall verdict:** Solid foundation. Ship v0.0.1 as a stdio-only MVP, but do not add a second transport or expose the binary to the network without first addressing the P1 items below.

---

## 1. Architecture & Design

### 1.1 Module boundaries
The crate is split cleanly into library (`src/lib.rs`) + binary (`src/main.rs`), which lets integration tests call tool handlers directly without subprocess overhead. This is a good trade: it makes tests fast and debuggable, while `tests/contract.rs` still locks in the JSON-RPC wire shape independently.

Module responsibilities are well-separated:
- `config.rs` — env-only, no file parsing, no runtime reconfiguration.
- `db.rs` — Postgres operations, migration runner, idempotency intercept.
- `embedding.rs` — ONNX + tokenizer, CPU-bound work dispatched via `spawn_blocking`.
- `error.rs` — exhaustive `ChittaError` enum with mandatory `tool`/`constraint`/`next_action` fields.
- `tools/{get,store,search}.rs` — thin handlers: validate → call db/embed → build output.
- `mcp.rs` — rmcp wiring, error translation, nothing else.

### 1.2 Type flow
The pipeline from wire JSON → database → wire JSON is type-checked end-to-end:
```
Wire → serde/schemars → Args → validate → MemoryRow → sqlx → Postgres
                                    ↑
Wire ← serde ← Output ← row_to_output ← MemoryRow ← sqlx ← Postgres
```
There are no hidden DTOs or mapper crates. `MemoryRow` derives `FromRow`; `StoreArgs` derives `Deserialize` + `JsonSchema`. This single-source-of-truth approach eliminates field-drift risk.

### 1.3 Concurrency model
v0.0.1 is stdio-only, so requests are naturally serialized. The code is already forward-looking:
- `PgPool` is used (not a single connection) so the HTTP transition doesn't need a rewrite.
- `Embedder::embed` runs inside `tokio::task::spawn_blocking` so the async runtime isn't blocked on CPU-bound inference.
- The `Mutex<Session>` is explicitly documented as a temporary bottleneck. The doc comment in `embedding.rs` is a model of responsible engineering: it names the exact `ort` version that causes the problem, describes three remediation paths, and warns not to add a second transport without revisiting it.

### 1.4 Design smells
- **Boilerplate in `mcp.rs`:** The three tool handlers (`store_memory`, `get_memory`, `search_memories`) follow identical `handle → map_err(chitta_to_rmcp) → serde_json::to_string_pretty → map_err(json_to_rmcp)` choreography. At three tools this is fine; at five it should be extracted into a helper macro or wrapper trait.
- **`Envelope::new` budget parameter is misleading:** `search.rs` calls `Envelope::new(..., 0)` and then immediately overwrites `envelope.budget_spent_tokens = estimate_tokens(&envelope)`. The `budget` argument is dead on arrival. Consider removing it from `new` and setting the field directly, or computing the estimate inside `new` when `T: Serialize`.

---

## 2. Code Quality & Safety

### 2.1 Error handling (excellent)
`ChittaError` is the standout feature of this codebase. Every variant carries enough context to build the Principle 8 contract (`tool`, `constraint`, `next_action`). The `db_next_action` helper disambiguates transient vs. permanent database failures, and the exhaustive `every_variant_populates_contract` test means adding a new variant without wiring it into `data()` fails at build or test time.

One minor concern: `serde_json::to_value(e.data()).ok()` in `mcp.rs:110` silently drops the structured payload if serialization fails. `ErrorData` only contains `String`, `&'static str`, and `Option<serde_json::Value>` fields, so failure is practically impossible, but an `expect` or explicit `match` would be more honest than `.ok()`.

### 2.2 Unsafe code
There is no production `unsafe`. The only `unsafe` blocks are in `config.rs` tests, where `std::env::set_var/remove_var` is wrapped behind a module-static `Mutex` and `catch_unwind`. This is the correct way to mutate environment variables in Rust 2024, and the comment clearly explains the invariant.

### 2.3 Panic safety
- **ONNX session mutex poisoning:** A panic inside `session.run()` (e.g., from a corrupted model) poisons the `Mutex` permanently. All subsequent `embed()` calls fail. The server process stays alive but two of three tools are broken. The v0.0.2 roadmap already lists this; the fix is either `catch_unwind` inside `spawn_blocking` or a session pool that discards poisoned sessions.
- **Transaction leak on panic:** `db::search_by_embedding` opens a transaction, runs `SET LOCAL`, executes the ANN query, and commits. A panic between `begin` and `commit` leaves the connection in an open transaction. sqlx returns it to the pool uncleaned. After `max_connections` such panics the pool is exhausted. This is a low-probability, high-severity issue that should be fixed before HTTP transport exposes the server to malformed or adversarial requests.

### 2.4 Input validation
Validation is centralized in `tools/validate.rs` and is thorough:
- Profile length, charset, and tag count/length are all bounded.
- `event_time` is sanity-checked against epoch and now+365d.
- `k` is clamped to `[1, 200]`.
- `min_similarity` rejects NaN and infinity.
- UUID parsing produces a caller-friendly error with the parse failure reason.

**Gap:** `content` is only validated for non-emptiness and token count. There is no byte-length sanity cap before tokenization. A malicious (or buggy) MCP client could send a multi-gigabyte string. Tokenization would either OOM or take unbounded time. A defense-in-depth cap (e.g., 1 MB or 10 MB) should be added to `validate::content_non_empty` or `validate::content_length`.

---

## 3. Testing

### 3.1 Unit / contract tests (strong)
- `error.rs`: 9 tests covering every variant's wire serialization, code routing, and `db_next_action` logic.
- `validate.rs`: 8 tests for every validation rule, including multibyte Unicode (the byte-vs-char bug was already caught and fixed).
- `envelope.rs`: 4 tests for envelope shape, null round-tripping, and token estimation monotonicity.
- `search.rs`: 5 tests for `prefix_chars` truncation and `apply_budget` behavior under tight, ample, and no-cap conditions.
- `config.rs`: 2 tests for missing `DATABASE_URL` and default path resolution, both using the safe `with_env` helper.
- `contract.rs`: 10 tests locking in serde shapes and the `chitta_to_rmcp` mapper. These are high-value tests: they will catch accidental field renames or type changes before any integration test runs.

### 3.2 Integration tests (good coverage, one gap)
`tests/integration.rs` exercises the full stack (Postgres + ONNX) with 8 tests:
- Idempotent replay (same row returned, exactly one DB row).
- Verbatim roundtrip with Unicode and whitespace.
- Empty-profile search envelope shape.
- `max_tokens` truncation.
- Invalid `event_time` error contract.
- `not_found` points at `search_memories`.
- Snippet is a verbatim 200-char prefix.
- Profile isolation (search in profile B doesn't see profile A).
- Content-too-long rejection with token count.
- Concurrent duplicate writes converge on one row.
- Semantic search finds the stored memory.

**Testing gaps identified:**
1. **Tag filtering:** The SQL branch `tags && $2` is never exercised with a non-empty tag array in integration tests.
2. **`min_similarity` > 0:** The Postgres predicate `(1.0 - (embedding <=> $2))::real >= $4` is never bound to a non-zero value in tests.
3. **Cross-profile `get_memory` isolation:** No test verifies that fetching a UUID from profile A under profile B returns `not_found`.
4. **MCP wiring layer:** All integration tests call `tools::store::handle` directly. The `mcp.rs` framing, routing, and `ServerHandler` impl are untested. A single in-process MCP client handshake test would close this gap without the fragility of a subprocess harness.

### 3.3 CI considerations
Integration tests skip silently when `TEST_DATABASE_URL` is unset. This is user-friendly for local `cargo test`, but it means a CI job that forgets to set the variable will pass while exercising zero database paths. Consider adding a `#[ignore]` attribute or a build-script probe so that CI without Postgres is explicit rather than silent.

---

## 4. Performance

### 4.1 Embedding cost on replay
`store_memory` embeds the content *before* checking idempotency. On an idempotent replay, the ONNX inference cost (~50–500 ms on CPU) is paid and then discarded. The code comment correctly explains why: on the cold-write path, a pre-flight `SELECT` would add a pointless round-trip. For stdio this is acceptable; for high-replay HTTP workloads it is not. The v0.0.2 roadmap already flags this.

### 4.2 Search truncation logic
`apply_budget` seeds its overhead calculation with an empty envelope whose `total_available` is `None`. The real envelope sets `total_available` to `Some(n)`. The delta is a few bytes, but for very tight `max_tokens` caps it means the truncation boundary is slightly profile-size-dependent. This is a minor correctness bug, not a performance issue.

### 4.3 `hnsw.ef_search` scaling
The `ef_search` floor is 200 and scales with `k * 4`, clamped at 1000. This is a reasonable heuristic for pgvector's HNSW index, but it is hard-coded. Making it configurable via environment variable (e.g., `CHITTA_HNSW_EF_SEARCH_MIN`) would let operators tune recall vs. latency without recompiling.

---

## 5. Security

### 5.1 SQL injection surface
The only dynamic SQL in the crate is:
```rust
sqlx::query(&format!("set local hnsw.ef_search = {ef_search}"))
```
in `db.rs:204`. The value is clamped to `[200, 1000]` and `k` is validated to `[1, 200]` before this point, so an injection via the public API is impossible today. However, `search_by_embedding` is a `pub` function in a library module. A future internal caller that skips `validate::k()` could pass an unclamped value directly into the SQL string.

**Recommendation:** Add a `debug_assert!` inside `search_by_embedding` that `ef_search` is within the expected range, or change the function visibility to `pub(crate)` to narrow the blast radius.

### 5.2 Resource exhaustion
- **No content byte-length cap:** As noted in §2.4, a huge `content` string can exhaust memory during tokenization.
- **No `spawn_blocking` timeout:** A stalled ONNX runtime blocks the thread forever. With the default tokio blocking pool (512 threads) this is unlikely to cascade, but it is unbounded.
- **No rate limiting:** Acceptable for stdio; mandatory for any network-facing transport.

### 5.3 Secrets in error payloads
Database error messages from `sqlx::Error::Database` are forwarded into the `received` field of the JSON-RPC error data. For stdio this is fine (the client is local), but before HTTP transport ships, audit whether Postgres error text can contain connection strings or table names that should not leave the server.

---

## 6. Documentation

### 6.1 Inline documentation (excellent)
Every module and every non-trivial function has a doc comment that explains *why*, not just *what*. Notable examples:
- `embedding.rs` module doc: explains the concurrency caveat, the model export details, and the exact tensor shapes.
- `db.rs` module doc: justifies runtime-checked queries (`sqlx::query`) over compile-time macros to avoid build friction.
- `config.rs` test helper: explains the `unsafe` invariant and the `Mutex` serialization strategy.

### 6.2 Markdown docs
The `docs/` tree is comprehensive:
- `principles.md` — 10 numbered design invariants that override convenience.
- `starting-shape.md` — schema, tool surface, dependency rationale, and explicit "out of scope" list.
- `architecture.md` — module diagram, startup sequence, request lifecycle, concurrency model.
- `data-model.md` — column reference, embedding pipeline, content-length policy.
- `errors.md` — error shape, JSON-RPC codes, rescue map, design invariants.
- `v0.0.2-roadmap.md` — Phased plan with specific numbered items and acceptance criteria.

This is among the best project documentation I have reviewed. The principles are not aspirational; they are enforced by tests (e.g., `every_variant_populates_contract`) and referenced in code comments.

### 6.3 Minor doc drift
- `Cargo.toml` says `clap` is needed for `--version`; `--http` is stubbed. The README says the same. Good.
- `tracing-subscriber` enables the `json` feature, but `main.rs` uses `tracing_subscriber::fmt()` (text format). The JSON capability is unused. Not a bug, but either enable it conditionally via an env var or remove the feature to shorten compile times slightly.

---

## 7. Dependency Management

Each dependency in `Cargo.toml` has a one-line comment stating its purpose (Principle 10). This is exemplary.

**Notable choices:**
- `ort = "=2.0.0-rc.10"` is pinned to an exact release candidate. This is appropriate because ONNX Runtime bindings are sensitive to ABI compatibility, but it means the project is tied to a pre-release API. Monitor `ort` stable releases.
- `rmcp = "0.8"` is a young crate. The MCP spec is itself evolving. The code abstracts rmcp behind `mcp.rs`, so a future rmcp 0.9 breakage would be contained to one file.
- `sqlx` uses runtime-checked queries rather than `query!` macros. This trades compile-time SQL validation for the ability to `cargo build` on a fresh clone without a live database. The rationale is documented in `starting-shape.md § sqlx mode`. This is the right call for an open-source project where contributors may not have Postgres running.

---

## 8. Detailed Findings

### P1 — Critical (fix before any network exposure)

| # | File | Issue | Impact | Recommended Fix |
|---|------|-------|--------|-----------------|
| 1 | `src/embedding.rs:132` | `Mutex<Session>` poisoned forever by ONNX panic | Server alive but 2/3 tools broken until restart | `catch_unwind` inside `spawn_blocking`, or session pool |
| 2 | `src/db.rs:200-204` | Panic inside `search_by_embedding` transaction leaks connection to pool | Pool exhaustion after `max_connections` panics | `catch_unwind` around transaction body, or explicit `tx.rollback()` in drop guard |
| 3 | `src/tools/validate.rs` | No byte-length cap on `content` before tokenization | OOM or unbounded CPU from huge input | Add `validate::content_byte_length(tool, value, max_bytes)` |
| 4 | `src/db.rs:204` | `hnsw.ef_search` formatted into SQL string via `format!` | Injection risk if clamping is bypassed by future internal caller | `debug_assert!(ef_search >= 200 && ef_search <= 1000)` inside `search_by_embedding`, or narrow visibility to `pub(crate)` |

### P2 — Moderate (fix in v0.0.2)

| # | File | Issue | Impact | Recommended Fix |
|---|------|-------|--------|-----------------|
| 5 | `src/tools/store.rs:85` | Embedding computed before idempotency check | Wasted CPU on replay (~50–500 ms per call) | Pre-flight `SELECT` on `(profile, idempotency_key)` before `embed()` |
| 6 | `src/db.rs:185-196` | `COUNT(*)` query outside transaction | MVCC skew: `total_available` may not match result set under concurrent writes | Move count query inside transaction, or use `count(*) OVER ()` in ANN query |
| 7 | `src/tools/search.rs:127-129` | `truncated` false positive when `min_similarity` filters out rows | `truncated=true` spuriously set; agents may pagination-loop | Only set `truncated=true` when `results.len() == k` (LIMIT was reached), or filter count by similarity |
| 8 | `src/tools/search.rs:156` | `apply_budget` seeds overhead with `total_available: None` | Budget cap is off by a few bytes for tight `max_tokens` | Seed overhead with the same `Some(total)` envelope shape, or compute delta explicitly |
| 9 | `src/embedding.rs` | `spawn_blocking` has no timeout | Thread blocked forever on stalled ORT | Wrap in `tokio::time::timeout` with a generous bound (e.g., 60s) |
| 10 | `src/mcp.rs:110` | `serde_json::to_value(e.data()).ok()` silently drops serialization failures | Structured error context lost on extremely rare serde failure | Use `expect` or explicit `match` and map to internal error |
| 11 | `src/config.rs:73-74` | `default_model_path()` falls back to relative path if `HOME` unset | Model resolution depends on process cwd; fragile in production | Return `Err` or use `std::env::current_dir()` with a loud warning |

### P3 — Low / Advisory

| # | File | Issue | Note |
|---|------|-------|------|
| 12 | `src/tools/search.rs:62` | `SearchHit` shadows `db::SearchHit` | Confusing but harmless; rename `db::SearchHit` to `db::SearchRow` before module grows |
| 13 | `src/mcp.rs:48-81` | Identical handle+serialize+map_err boilerplate across 3 tools | Extract helper when tool count reaches 5+ |
| 14 | `src/envelope.rs:19` | `Envelope::new` `budget` argument is always overwritten | Remove parameter or compute estimate inside `new` |
| 15 | `src/main.rs:37-42` | `tracing-subscriber` `json` feature enabled but unused | Either add `--log-format=json` CLI flag, or remove feature from `Cargo.toml` |
| 16 | `tests/integration.rs:76-83` | Model-path default logic duplicated from `config.rs` | Expose `default_model_path()` or use `Config::from_env()` in test harness |
| 17 | `src/db.rs` | DB functions lack `#[tracing::instrument]` spans | Add spans so search latency and pool acquisition time are visible in traces |
| 18 | `src/embedding.rs:74` | `GraphOptimizationLevel::Level1` | Level3 may be 10–40% faster; validate output similarity within tolerance, then upgrade |

---

## 9. Testing Recommendations

1. **Add a non-empty tag search integration test.** Verify that `tags: ["db"]` filters results correctly and that OR-match semantics hold.
2. **Add a `min_similarity` integration test.** Seed a profile with related and unrelated memories; assert that a high `min_similarity` floor drops the unrelated ones and does not spuriously set `truncated=true`.
3. **Add an MCP handshake test.** Spawn an in-process `ChittaServer`, send a single `tools/list` request via rmcp's client API, and assert the three tools are advertised. This validates the wiring layer without subprocess overhead.
4. **Add a cross-profile isolation test for `get_memory`.** Store a memory in profile A, then call `get_memory` with the same UUID but profile B, asserting `not_found`.

---

## 10. Conclusion

chitta-rs v0.0.1 is a **high-quality MVP** with a clear thesis: small core, grow by evidence, and never ship an error message that doesn't tell the agent what to do next. The code is clean, the tests are meaningful, and the documentation sets a standard that larger projects should emulate.

The issues found are almost all architectural guardrails for v0.0.2 (concurrency, transport, scale), not bugs in the v0.0.1 stdio-only surface. The one exception is the missing byte-length cap on `content`, which is a small addition that significantly hardens the server against adversarial input.

**Recommended action:** Merge and tag v0.0.1 as-is for stdio MCP use. Begin v0.0.2 with the P1 items (mutex poisoning, transaction panic safety, content length cap, SQL formatting guard) before any HTTP transport work.
