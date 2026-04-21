# chitta-rs v0.0.1 code review findings

Review date: 2026-04-20
Branch: `research` (merge-base with `main`)
Scope: `rust/` directory — 22 files, 3,497 lines
Reviewers: correctness, testing, maintainability, security, performance, api-contract, reliability, adversarial

## Applied fixes

| # | Severity | File | Issue | Fix |
|---|----------|------|-------|-----|
| 1 | P1 | `src/tools/validate.rs:106` | Tag length used `.len()` (bytes) not `.chars().count()` — multi-byte tags incorrectly rejected | Changed to `.chars().count()`, consistent with all other validators |
| 16 | P3 | `src/tools/search.rs:134` | `total_available as u64` unchecked — silent wrapping if count negative | `u64::try_from().unwrap_or(0)` |
| 2 | P1 | `src/db.rs:40` | No connect timeout on pool — startup blocks indefinitely if Postgres is down | Added `acquire_timeout(5s)`, `idle_timeout(600s)`, configurable via env |
| 6 | P2 | `src/embedding.rs:177` | `embed()` hardcoded `tool: "store_memory"` — wrong tool name in errors from search | Threaded `tool: &'static str` through `embed()` and `ort_to_embed_err()` |
| 12 | P2 | `src/db.rs:41` | Pool hardcoded at 8, no idle/acquire timeouts | Combined with #2 — `CHITTA_DB_MAX_CONNECTIONS`, `CHITTA_DB_ACQUIRE_TIMEOUT`, `CHITTA_DB_IDLE_TIMEOUT` |

## Deferred findings (fix in v0.0.2)

### P1 — high

**#3 — ONNX session panic poisons Mutex permanently**
- File: `src/embedding.rs:128`
- Reviewers: correctness, reliability, adversarial (confidence 0.95)
- Impact: A panic inside `session.run()` poisons the Mutex. All subsequent `embed()` calls fail. `store_memory` and `search_memories` are permanently dead; only `get_memory` survives. Process stays alive but two of three tools are broken until restart.
- Suggested fix: Wrap `session.run()` in `std::panic::catch_unwind` inside the `spawn_blocking` closure, or accept that ORT panics are fatal and document restart-on-failure.

**#4 — Token-length guard bypassed if tokenizer.json has truncation enabled**
- File: `src/embedding.rs:128`
- Reviewer: adversarial (confidence 0.80)
- Impact: If the deployed `tokenizer.json` has truncation enabled at `max_length=8192`, `ids.len()` is always <= 8192 and the `> MAX_TOKENS` check never fires. A 10,000-token document gets stored verbatim but embedded as its first 8192 tokens. Violates Principle 1 (never embed a truncated version of stored content) silently.
- Suggested fix: Check the tokenizer's truncation config at `Embedder::load` time and either disable it or fail loudly.

**#5 — Panic inside search transaction leaks connection, 8 panics exhausts pool**
- File: `src/db.rs:183`
- Reviewer: adversarial (confidence 0.82)
- Impact: A panic inside `pool.begin()`/`tx.commit()` leaves the connection in an open-transaction state. sqlx does not detect mid-transaction connections as unhealthy. After `max_connections` such panics, the pool is permanently exhausted.
- Suggested fix: Use `catch_unwind` around the transaction body, or rely on `acquire_timeout` (now added) to surface the problem rather than hang.

### P2 — moderate

**#7 — Count query outside transaction (MVCC skew)**
- File: `src/db.rs:183`
- Reviewers: correctness, performance, reliability, adversarial (confidence 0.92)
- Impact: The `COUNT(*)` query and the ANN query run in different snapshots. Concurrent writes between them produce `total_available` that doesn't match the result set, potentially setting `truncated=true` spuriously.
- Suggested fix: Move the count query inside the transaction, or use a window function `count(*) OVER ()` in the same query.

**#8 — truncated flag false positive from min_similarity**
- File: `src/tools/search.rs:127`
- Reviewer: correctness (confidence 0.90)
- Impact: `truncated` is set when `results.len() < total_available`, but `total_available` counts all profile memories regardless of `min_similarity`. A search with `min_similarity=0.9` against a large profile with mostly-dissimilar memories will always show `truncated=true`, potentially triggering agent pagination loops.
- Suggested fix: Either filter `total_available` by `min_similarity` (expensive), or change the `truncated` semantics to only reflect the `k`-cut (was the LIMIT binding reached?).

**#9 — apply_budget overhead seed mismatch**
- File: `src/tools/search.rs:156`
- Reviewers: correctness, testing, maintainability, adversarial (confidence 0.92)
- Impact: The empty-envelope overhead is seeded with `total_available: None` but the real envelope uses `Some(n)`. The delta is a few bytes but makes the truncation boundary profile-size-dependent for tight `max_tokens` caps.

**#10 — Embedding computed before idempotency check**
- File: `src/tools/store.rs:85`
- Reviewers: correctness, performance, adversarial (confidence 1.00)
- Impact: Every `store_memory` call pays the full ONNX inference cost (~50-500ms) before the `INSERT` attempt. On idempotent replay, the embedding is discarded. Acceptable for v0.0.1 stdio, problematic at high replay rates.
- Suggested fix: Pre-flight `SELECT` on idempotency_key before calling `embed()`.

**#11 — ef_search GUC formatted into SQL string**
- File: `src/db.rs:200`
- Reviewers: security, adversarial (confidence 0.92)
- Impact: The value is clamped to `[200, 1000]` so current callers are safe. But `search_by_embedding` is `pub(crate)` — a future caller that skips `validate::k()` would produce an unclamped value formatted directly into SQL.
- Suggested fix: Add a debug assertion on the clamped range, or make the function `pub(super)` / add an internal re-validation.

**#13 — ONNX session in spawn_blocking has no timeout**
- File: `src/embedding.rs:128`
- Reviewers: reliability, adversarial (confidence 0.88)
- Impact: A stalled ORT runtime blocks the `spawn_blocking` thread forever. With the default tokio blocking pool (512 threads) this is unlikely to cascade, but a pathological model could hang the process.

**#14 — Global Mutex serializes all concurrent requests (v0.0.2 blocker)**
- File: `src/embedding.rs:56`
- Reviewer: performance (confidence 0.85)
- Impact: Fine for v0.0.1 stdio (one request at a time). Becomes a total serialization bottleneck under any pipelining transport. BGE-M3 CPU inference at 50-500ms/call means 10 concurrent requests queue serially.
- Suggested fix: Session pool (N = num_cpus) behind Semaphore, dedicated embedder thread + mpsc channel, or an `ort` version that supports `&self` inference.

**#15 — SIGTERM races in-flight spawn_blocking tasks**
- File: `src/main.rs`
- Reviewer: adversarial (confidence 0.74)
- Impact: `tokio::select!` picks the shutdown arm immediately on SIGTERM. In-flight `spawn_blocking` tasks are cancelled or their results dropped. The MCP client receives EOF. Idempotency_key makes DB state safe on retry, but embed CPU is wasted.

## Advisory items (no code action needed)

| # | File | Issue | Note |
|---|------|-------|------|
| 17 | `src/db.rs:31` | Two public types named `SearchHit` in different modules | Rename `db::SearchHit` to `db::SearchRow` before the module grows |
| 18 | `src/error.rs:146` | Raw Postgres error text forwarded to MCP wire client | Acceptable for stdio; audit before HTTP transport |
| 19 | `src/mcp.rs:48` | Identical handle+serialize boilerplate 3x | Extract helper when tool count reaches 5+ |
| 20 | `tests/integration.rs:76` | Model-path default logic duplicated from config.rs | Use `Config::from_env()` or expose `default_model_path` |
| 21 | `src/embedding.rs:74` | GraphOptimizationLevel::Level1 — Level3 is 10-40% faster | Validate Level3 output similarity within tolerance, then upgrade |

## Testing gaps

- Tag-filter SQL branch (`tags && $3`) never exercised with non-empty tags
- `min_similarity` Postgres predicate never bound to a non-zero value
- `get_memory` cross-profile isolation not tested (UUID from profile A, lookup under profile B)
- Entire MCP wiring layer (`mcp.rs`) bypassed — all integration tests call tool handlers directly
- `validate::tags` had no multibyte character test (the byte-vs-char bug was invisible)
- Integration tests skip silently without `TEST_DATABASE_URL` — CI without it exercises zero DB paths

## Contract compatibility (cutover planning)

The Rust version intentionally diverges from the Python version's MCP tool contracts. All three tools have breaking changes that require a cutover migration:

| Tool | Breaking change |
|------|----------------|
| `store_memory` | `profile` and `idempotency_key` now required; `source`, `metadata`, `auto_link` dropped |
| `store_memory` | Response drops `source`, `links_created`; adds `event_time`, `record_time`, `idempotent_replay` |
| `get_memory` | Parameter renamed `memory_id` → `id`; `profile` promoted to required |
| `get_memory` | Not-found moved from in-band `{memory: null}` to JSON-RPC error |
| `search_memories` | `limit` → `k`; `source`/`graph_depth`/`profiles` dropped |
| `search_memories` | Envelope: `budget_spent` → `budget_spent_tokens`; `refinement_advice` dropped |
| `search_memories` | Hit: `score` → `similarity`; `title`/`entities` removed |

None of these are versioned or behind a flag. Cutover requires updating every call site before switching.
