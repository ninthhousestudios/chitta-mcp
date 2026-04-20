# chitta-rs v0.0.1 — starting shape

Reference for the first release. Captures the schema, tool surface, wire contract, dependencies, and config. Everything here is the concrete manifestation of `rust/docs/principles.md`; if these docs disagree, principles win and this doc is wrong.

---

## Scope

v0.0.1 is the smallest thing that replaces everyday use of Python chitta.

**In scope:**
- Single binary `chitta-rs`, stdio MCP transport only.
- Three tools: `store_memory`, `get_memory`, `search_memories`.
- Postgres + pgvector backend.
- Local BGE-M3 embeddings via ONNX, reusing `~/.cache/chitta/bge-m3-onnx`.
- Bi-temporal (`event_time` + `record_time`) from the first row.
- Idempotent writes.
- Agent-native response envelope on `search_memories`.
- Shape-conformant, actionable errors.

**Explicitly out of scope (until a benchmark or documented need earns it):**
- HTTP / SSE / remote transport (v0.0.2+).
- Entity extraction, temporal parsing, language packs.
- Full-text search (`fts`), sparse embeddings, SimHash.
- PageRank, co-occurrence, FSRS, Hebbian decay, confidence, importance, access counts.
- Memory relationships / knowledge graph.
- Contradictions, invalidation, TTL, compression.
- Export/import, batch ingest, audit log, profile settings.
- `metadata jsonb` column (deferred until an actual shape is needed).

---

## Database

### DB name

`chitta_rs` (Postgres convention — underscores). Lives alongside the legacy Python DB; they do not share data.

### Extension

```sql
create extension if not exists vector;
```

That's it. Just pgvector.

### `memories` table

```sql
create table memories (
    id                uuid        primary key,
    profile           text        not null,
    content           text        not null,
    embedding         vector(1024) not null,
    event_time        timestamptz not null,
    record_time       timestamptz not null default now(),
    tags              text[]      not null default '{}',
    idempotency_key   text        not null
);
```

**Columns in English:**

- `id` — UUID v7. Time-sortable; generated client-side (in the Rust server, before insert). No `gen_random_uuid()` default — we want v7, not v4.
- `profile` — the namespace. Required on every write. Principle 7.
- `content` — the verbatim memory text. Immutable. Principle 1.
- `embedding` — 1024-dim dense vector from BGE-M3. Not null because we embed before insert.
- `event_time` — when the memory's subject happened. Client may supply; if omitted, server sets it equal to `record_time`. Principle 2.
- `record_time` — when chitta-rs learned of it. Server-set. Never changes. Principle 2.
- `tags` — array of short string labels. Empty array default; never null.
- `idempotency_key` — required on every write. Used with `profile` to detect duplicate submissions. Principle 6.

### Indexes

```sql
-- ANN search on embeddings. HNSW with cosine distance.
create index memories_embedding_idx
    on memories using hnsw (embedding vector_cosine_ops)
    with (m = 16, ef_construction = 64);

-- Profile-scoped recent-first listing (and for record_time-ordered queries).
create index memories_profile_record_time_idx
    on memories (profile, record_time desc);

-- Tag filtering.
create index memories_tags_idx
    on memories using gin (tags);

-- Idempotency: one key per profile.
create unique index memories_profile_idempotency_key_uniq
    on memories (profile, idempotency_key);
```

Four indexes. Each ties to a query we actually run:

- **HNSW on embedding** — `search_memories` ANN lookup. `m=16, ef_construction=64` are pgvector's reasonable defaults; we'll revisit when we have >100k rows.
- **(profile, record_time desc)** — generic filter-by-profile path; also lets us do `ORDER BY record_time DESC` without a sort step.
- **GIN on tags** — tag-filter clause in `search_memories`.
- **Unique (profile, idempotency_key)** — enforces idempotent writes; the constraint *is* the dedup mechanism.

No FTS index (no full-text search in v0.0.1). No sparse embedding index. No entity or relationship tables.

---

## Embedding pipeline

### Model

- Path: `$CHITTA_MODEL_PATH/bge_m3_model.onnx` (default `~/.cache/chitta/bge-m3-onnx/bge_m3_model.onnx`).
- External weight sidecar: `bge_m3_model.onnx_data` must live next to the `.onnx` file. `ort` resolves it automatically when loading by file path.
- Tokenizer: `$CHITTA_MODEL_PATH/tokenizer.json` (HuggingFace fast-tokenizer format).

One `ort::Session` is constructed at startup, wrapped in `Arc`, and shared across all requests. No per-request session construction. No model reload without restart.

### Pipeline

Per text input:

1. Tokenize with the HF tokenizer. Produces `input_ids`, `attention_mask`, `token_type_ids`.
2. Reject if token count > 8192 (see Content length policy below).
3. Run the ONNX session. Output tensor shape: `[1, seq_len, 1024]`.
4. Pool by taking the **CLS token hidden state** (`output[0, 0, :]`). BGE-M3's trained dense-retrieval head uses CLS pooling.
5. L2-normalize the 1024-dim vector. Store as `pgvector::Vector`.

### Output

- Dimension: **1024** (matches `vector(1024)` in the `memories` table).
- Deterministic on single-threaded CPU execution. The integration test that asserts equality across two calls pins ORT to single-thread intra-op for that test; production can use default threading.

### Content length policy

BGE-M3 has a hard 8192-token context. Two reasons to reject rather than truncate:

1. **Principle 1.** The stored `content` is verbatim. If we silently embed only the first 8192 tokens of a 20k-token memory, the embedding lies about what's stored — the tail is unsearchable and the caller never knows.
2. **Chunking strategy belongs to the caller.** Paragraph-aware, markdown-aware, semantic, sliding-window — different clients want different splits. Server-side chunking is a real feature that earns its place in a later release with its own design and benchmark (Principle 5).

v0.0.1 rejects over-long content at the `store_memory` validate step. The caller decides how to split and calls `store_memory` N times with N distinct idempotency keys.

Error shape (subset of the general error contract):

```json
{
  "code": -32602,
  "message": "content exceeds 8192-token embedding limit",
  "data": {
    "tool": "store_memory",
    "argument": "content",
    "constraint": "tokenized length <= 8192",
    "received": { "token_count": 11432 },
    "next_action": "Split content into chunks of <= 7500 tokens and store each as a separate memory with its own idempotency_key"
  }
}
```

---

## Tool surface

### Wire envelope (retrieval only)

Every retrieval tool returns the standard envelope. Principle 4.

```json
{
  "results": [...],
  "truncated": false,
  "total_available": 42,
  "budget_spent_tokens": 1200
}
```

- `results` — the actual items (shape tool-specific).
- `truncated` — `true` if `max_tokens` or `k` cut the list short.
- `total_available` — how many rows matched before truncation. `null` if the tool can't cheaply count.
- `budget_spent_tokens` — estimated token count of the response payload. Estimation rule: `ceil(response_bytes / 4)`. Documented as approximate; tightened when we have a real tokenizer on the hot path.

Writes do not use this envelope — they return a single record.

### Error shape

Every error uses JSON-RPC 2.0 with a populated `data` field. Principle 8.

```json
{
  "code": -32602,
  "message": "event_time is before 1970-01-01",
  "data": {
    "tool": "store_memory",
    "argument": "event_time",
    "constraint": "ISO-8601 timestamp >= 1970-01-01T00:00:00Z",
    "received": "1969-06-20T00:00:00Z",
    "next_action": "Pass event_time >= 1970-01-01T00:00:00Z, or omit to default to record_time"
  }
}
```

Mandatory `data` fields: `tool`, `constraint`, `next_action`. Optional: `argument`, `received`, any tool-specific diagnostic keys.

### Error & rescue map

Every external boundary must translate its native errors to `ChittaError` variants that carry actionable `data`. No `unwrap()`, no `rescue StandardError`, no leaked stack traces.

| Codepath | Failure mode | Native error | Rescue / translation | Result |
|---|---|---|---|---|
| `db::connect` (startup) | Postgres unreachable, bad URL | `sqlx::Error::Io`, `PoolTimedOut` | log + exit 1; message names `DATABASE_URL` | fatal-at-startup, stderr only |
| `sqlx::migrate!` (startup) | schema drift / corrupt migration state | `MigrateError` | log + exit 1 | fatal-at-startup |
| `insert_memory` | unique violation on `(profile, idempotency_key)` | pg `23505` | **intercept**: fetch the existing row and return with `idempotent_replay: true` | happy path (concurrent duplicate writers) |
| `insert_memory` | pool exhausted / conn reset mid-request | `PoolTimedOut`, `Io` | `ChittaError::Db` with `next_action: "Retry the request. If the error repeats, check server load or database availability."` | tool-level JSON-RPC error |
| `embed` | tokenized length > 8192 | — (our check) | `ChittaError::ContentTooLong` with token count + limit in `received` | see Embedding pipeline § Content length policy |
| `embed` | tokenizer fails (malformed input) | `tokenizers::Error` | `ChittaError::Embedding` with generic `next_action: "Ensure content is valid UTF-8."` | tool-level error |
| `embed` | ORT session output wrong shape / NaN | `ort::Error` or shape mismatch | `ChittaError::Embedding` with `next_action: "Report this as a bug; include server logs."` | tool-level error |
| stdio transport | client disconnect | broken pipe / EOF on stdin | graceful shutdown via SIGINT/SIGTERM handler; in-flight request cancelled | N/A (server exits cleanly) |
| stdio transport | malformed JSON-RPC frame | handled by `rmcp` | inherit `rmcp` default | JSON-RPC `-32700` parse error |

**Rule:** Any new external call added later must extend this table in the same PR.

### `store_memory`

Insert a new memory.

**Args:**
| name | type | required | notes |
|---|---|---|---|
| `profile` | string | yes | Target profile / namespace. 1–128 chars, `[a-zA-Z0-9_-]+` only. |
| `content` | string | yes | Verbatim text. Stored as-is. Min 1 char. Max enforced via token count — see Embedding pipeline § Content length policy. |
| `idempotency_key` | string | yes | Client-supplied. 1–128 chars, no control characters. Duplicate key in the same profile returns the prior row. |
| `event_time` | string (ISO-8601) | no | When it happened. Must be ≥ 1970-01-01T00:00:00Z and ≤ `now() + 365 days` (sanity bound, not a hard contract). Defaults to `record_time`. |
| `tags` | array of strings | no | Short labels. Each tag 1–64 chars. Max 32 tags. Defaults to `[]`. |

Every validation failure returns a JSON-RPC error with `tool`, `argument`, `constraint`, `received`, `next_action` populated per the Error shape section.

**Returns:**
```json
{
  "id": "018f5a9c-…",
  "profile": "default",
  "content": "…",
  "event_time": "2026-04-19T10:12:00Z",
  "record_time": "2026-04-19T10:12:00Z",
  "tags": ["project:chitta"],
  "idempotent_replay": false
}
```

- `idempotent_replay: true` when the `(profile, idempotency_key)` pair matched an existing row. The returned record is the pre-existing one; no new row was created; no new embedding computed.

### `get_memory`

Fetch a memory by id.

**Args:**
| name | type | required |
|---|---|---|
| `profile` | string | yes |
| `id` | string (uuid) | yes |

**Returns:** the full memory record, same shape as `store_memory`'s return (minus `idempotent_replay`).

**Errors:** `not_found` if `(profile, id)` does not match a row. `next_action` suggests checking the profile or calling `search_memories`.

### `search_memories`

Semantic similarity search with outline-style results. Principle 4.

**Args:**
| name | type | required | default |
|---|---|---|---|
| `profile` | string | yes | — |
| `query` | string | yes | — |
| `k` | integer | no | 10 |
| `max_tokens` | integer | no | unbounded |
| `tags` | array of strings | no | null (no filter) |
| `min_similarity` | float | no | 0.0 |

Tag match is OR: a memory matches if it has *any* of the supplied tags.

**Returns:** the standard envelope. Each result item:
```json
{
  "id": "018f5a9c-…",
  "snippet": "first 200 chars of content, no tail ellipsis inside the 200",
  "similarity": 0.87,
  "event_time": "2026-04-10T14:00:00Z",
  "record_time": "2026-04-10T14:01:00Z",
  "tags": ["project:chitta"]
}
```

- `snippet` — first 200 chars of `content`, verbatim. If `content` is ≤200 chars, the snippet equals the content. No ellipsis affordance in v0.0.1; the client can infer truncation from `snippet.length == 200 && get_memory(id).content.length > 200`.
- `similarity` — cosine similarity in `[0.0, 1.0]` (pgvector's `<=>` returns distance; we convert).
- Full `content` is **never** in search results. To read the body, call `get_memory(id)`.

**Truncation rules:**
- Hard cap at `k` results.
- If `max_tokens` is set, stop appending results when the next result would push the *full envelope's* `budget_spent_tokens` over the cap (envelope wrapper overhead is counted, not just hit payloads). Set `truncated: true`.
- `total_available` is the count of rows matching `profile` + `tags` filter — it deliberately **ignores `min_similarity`**. Counting rows above a cosine threshold would require scanning every embedding, defeating the ANN index. The caller gets a truthful ceiling on candidate breadth; `results` reports the similarity-gated subset, with `truncated` set when the HNSW+floor combination trimmed below `total_available`.
- pgvector's `hnsw.ef_search` is raised per-query (floor 200, scaled with `k`) so filter post-selection does not silently shrink result count. The setting is scoped to a single transaction — it does not leak across pool connections.

---

## Dependencies

`Cargo.toml` gets one comment per dep per Principle 10.

```toml
[dependencies]
# Async runtime. De facto standard for Rust network services.
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal"] }

# Official MCP SDK. Speaks protocol and transport; we implement handlers.
rmcp = { version = "...", features = ["server", "transport-io"] }

# Postgres driver. Runtime-checked queries in v0.0.1 (see Build & runtime § sqlx mode).
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "migrate"] }

# pgvector integration for sqlx + serde.
pgvector = { version = "0.4", features = ["sqlx", "serde"] }

# UUID generation. V7 = time-sortable, which matters for record_time ordering.
uuid = { version = "1", features = ["v7", "serde"] }

# Timestamps mapped to Postgres timestamptz.
chrono = { version = "0.4", features = ["serde"] }

# Canonical JSON (de)serialization; required by MCP wire format.
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# ONNX Runtime bindings. Runs the BGE-M3 model we already have on disk.
ort = { version = "2", default-features = false, features = ["load-dynamic"] }

# HuggingFace tokenizer. Loads BGE-M3's `tokenizer.json` fast-tokenizer file.
tokenizers = "0.20"

# .env loader at startup. No magic, no global mutation.
dotenvy = "0.15"

# Structured logging.
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Error types. thiserror for library errors, anyhow for main().
thiserror = "1"
anyhow = "1"

# CLI flag parsing. Needed once we add --http; cheap to include now.
clap = { version = "4", features = ["derive"] }
```

Versions shown are directional; pin exact versions in the actual `Cargo.toml` with `cargo add` and record them in the lockfile.

`rmcp` version is a placeholder until the crate is added — resolve via `cargo add rmcp --features server,transport-io` and pin to whatever stable release that yields. Same for any other `"..."` placeholders that survive to real `Cargo.toml`.

### sqlx mode: runtime-checked

v0.0.1 uses `sqlx::query()` and `sqlx::query_as()` (runtime-checked) rather than the `query!`/`query_as!` compile-time macros. Rationale:

- Compile-time macros require a live `DATABASE_URL` at `cargo build` time (or a committed `.sqlx/` offline cache maintained via `cargo sqlx prepare`). That makes fresh clones and CI fragile for v0.0.1.
- Runtime-checked queries still return strongly-typed rows via `query_as::<_, MemoryRow>(...)`.
- This is a deliberate trade: less compile-time safety now, zero build-from-fresh friction. Revisit in v0.1 if SQL regressions start shipping.

### ONNX Runtime provisioning

`ort` is configured with `features = ["load-dynamic"]` — it does **not** download or vendor the runtime. At process start, `ort` dlopen's `libonnxruntime.so` (Linux) / `libonnxruntime.dylib` (macOS) / `onnxruntime.dll` (Windows). The library must be discoverable by one of:

1. **System package.** Arch: `pacman -S onnxruntime`. Debian/Ubuntu: `apt install libonnxruntime-dev` (check distro naming). This is the default, documented path in the README.
2. **Explicit path.** Set `ORT_DYLIB_PATH=/absolute/path/to/libonnxruntime.so` in the environment. Takes precedence over system discovery.

Deliberate non-choices:
- **No `features = ["download-binaries"]`.** Makes builds network-dependent and hides what version is linked.
- **No vendoring in v0.0.1.** Adds build complexity (C++ toolchain, manylinux concerns). Can be added later if distribution becomes a friction point.

README must document the system-package install as a prerequisite.

---

## Configuration

All config via environment variables. `.env` loaded at startup.

| variable | required | default | purpose |
|---|---|---|---|
| `DATABASE_URL` | yes | — | Postgres connection string for the `chitta_rs` DB. |
| `CHITTA_MODEL_PATH` | no | `~/.cache/chitta/bge-m3-onnx` | Directory containing `bge_m3_model.onnx` + `tokenizer.json`. |
| `CHITTA_LOG_LEVEL` | no | `info` | Matches `tracing-subscriber` env-filter syntax. |

No config files. No runtime reconfiguration. Restart to change settings.

---

## Transport

v0.0.1 is **stdio only**. The binary reads MCP requests from stdin and writes responses to stdout; logs go to stderr.

```
chitta-rs                    # stdio, default
```

HTTP transport (with bearer-token auth) lands in v0.0.2. Same binary, `--http --bind … --auth-token-file …` flag.

---

## Out of scope — explicit list

These are not in v0.0.1. Each earns its place later only by winning a benchmark or a documented recurring need:

- Any extraction (entities, temporal, recurrence, anchors, quantities).
- Any YAML language packs / multilingual data files.
- Full-text search, sparse embeddings, hybrid RRF.
- SimHash dedup, near-duplicate detection beyond `idempotency_key`.
- Access counts, confidence, importance, surprise, compression_level.
- Expiry / TTL / `expires_at`.
- Memory graph: relationships, entities, contradictions, aliases.
- PageRank, co-occurrence, Hebbian decay, FSRS.
- Batch ingest, export/import, audit log, profile settings.
- HTTP transport, authentication.
- Any MCP tool beyond the three listed.

Deleting from the Python tree doesn't mean permanently removing these ideas — it means they re-enter the design through the release-gate process described in `docs/research/master-plan.md`, not through direct port.

---

## What v0.0.1 success looks like

One command starts the server; an MCP client connects over stdio. You call `store_memory` three times with the same idempotency key and get one row (two replays). You call `search_memories` and get snippets under a `max_tokens` budget, with `truncated` / `total_available` reported honestly. You call `get_memory(id)` and get the full content. You restart the server and your memories are still there.

That's it. Everything beyond that is a later release.
