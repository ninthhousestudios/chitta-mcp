# Data model

## Database schema

chitta-rs uses a single Postgres table with the pgvector extension. The schema is applied automatically at startup via sqlx migrations.

### The `memories` table

```sql
create table memories (
    id                uuid         primary key,
    profile           text         not null,
    content           text         not null,
    embedding         vector(1024) not null,
    event_time        timestamptz  not null,
    record_time       timestamptz  not null default now(),
    tags              text[]       not null default '{}',
    idempotency_key   text         not null
);
```

### Column reference

| Column | Type | Description |
|---|---|---|
| `id` | uuid | UUID v7, generated server-side before insert. Time-sortable -- the timestamp component reflects creation order. Not `gen_random_uuid()` (which produces v4). |
| `profile` | text | Namespace. Required on every write and query. A memory belongs to exactly one profile. |
| `content` | text | Verbatim memory text. Stored exactly as received. Never mutated after insert. |
| `embedding` | vector(1024) | 1024-dimensional dense vector from BGE-M3. Computed at write time, before insert. |
| `event_time` | timestamptz | When the subject happened in the world. Client may supply; defaults to `record_time` if omitted. |
| `record_time` | timestamptz | When chitta-rs received the memory. Server-set. Never changes. |
| `tags` | text[] | Array of short string labels. Empty array by default, never null. |
| `idempotency_key` | text | Client-supplied dedup key. Unique within a profile. |

### Indexes

Four indexes, each tied to a query the server actually runs:

```sql
-- ANN search. HNSW with cosine distance.
create index memories_embedding_idx
    on memories using hnsw (embedding vector_cosine_ops)
    with (m = 16, ef_construction = 64);

-- Profile-scoped recent-first listing.
create index memories_profile_record_time_idx
    on memories (profile, record_time desc);

-- Tag filtering (OR-match on any tag).
create index memories_tags_idx
    on memories using gin (tags);

-- Idempotency: one key per profile. This IS the dedup mechanism.
create unique index memories_profile_idempotency_key_uniq
    on memories (profile, idempotency_key);
```

| Index | Purpose |
|---|---|
| `memories_embedding_idx` | HNSW index for `search_memories` ANN lookup. `m=16, ef_construction=64` are pgvector defaults; suitable up to ~100k rows. |
| `memories_profile_record_time_idx` | B-tree on `(profile, record_time desc)`. Supports profile-scoped queries and ordering by record time without a sort step. |
| `memories_tags_idx` | GIN index on the `tags` array. Supports the `tags && $1` OR-match filter in search queries. |
| `memories_profile_idempotency_key_uniq` | Unique B-tree on `(profile, idempotency_key)`. Enforces idempotent writes. The constraint raises Postgres error `23505` on conflict, which the write path intercepts. |

---

## Bi-temporal model

Every memory has two timestamps:

- **`event_time`** -- when the thing happened in the world. The client can set this to any time >= 1970-01-01 and <= now + 365 days. If omitted, it defaults to `record_time`.
- **`record_time`** -- when chitta-rs learned about it. Always server-set to `now()` at insert time. Immutable.

This distinction matters for temporal reasoning. An agent might record a fact today about a meeting that happened yesterday. The `event_time` says "yesterday"; the `record_time` says "today." Both are preserved, so queries can filter or sort by either.

There are no updates. A correction is a new memory that supersedes the old one. The audit trail is append-only.

---

## Embedding pipeline

### Model: BGE-M3

chitta-rs uses [BGE-M3](https://huggingface.co/BAAI/bge-m3) (BAAI General Embedding, Multilingual, Multi-Functionality, Multi-Granularity) for dense vector embeddings. The model is run locally via ONNX Runtime -- no external API calls.

The specific export is from [yuniko-software/bge-m3-onnx](https://huggingface.co/yuniko-software/bge-m3-onnx). This export has CLS-token pooling and L2 normalization baked into the ONNX graph, so the host code does no post-processing. The named output is `dense_embeddings` with shape `[batch, 1024]`.

The `sparse_weights` output is ignored in v0.0.1 (no sparse search column in the schema).

### Model files

Default location: `~/.cache/chitta/bge-m3-onnx/`. Override with `CHITTA_MODEL_PATH`.

| File | Purpose |
|---|---|
| `bge_m3_model.onnx` | ONNX model graph |
| `bge_m3_model.onnx_data` | External weight sidecar. Must be adjacent to the `.onnx` file. `ort` resolves it automatically. |
| `tokenizer.json` | HuggingFace fast-tokenizer format. Used to tokenize text before inference. |

### Embedding steps

For each text input (content on store, query on search):

1. **Tokenize** using the HuggingFace tokenizer. Produces `input_ids` and `attention_mask`.
2. **Reject** if token count exceeds 8192 (see content length policy below).
3. **Run ONNX inference.** Input: `input_ids` and `attention_mask` as `[1, seq_len]` tensors. Output: `dense_embeddings` as `[1, 1024]`.
4. **Validate** output shape (must total 1024 elements).
5. **Return** the 1024-dimensional `Vec<f32>`, which is stored as a `pgvector::Vector`.

The ONNX session is loaded once at startup and shared via `Arc<Embedder>`. Embedding is CPU-bound and dispatched to `tokio::task::spawn_blocking` to avoid blocking the async runtime.

### Embedding dimension

**1024 dimensions.** This matches the `vector(1024)` column type in the database and BGE-M3's dense output. The constant is pinned as `EMBEDDING_DIM` in the code; a dimension mismatch would panic loudly rather than produce silent garbage.

### Content length policy

BGE-M3 has a hard 8192-token context window. chitta-rs rejects content that exceeds this limit rather than silently truncating. The reasons:

1. **Verbatim fidelity.** If the server embedded only the first 8192 tokens of a 20k-token memory, the embedding would misrepresent the stored content. The tail would be unsearchable and the caller would never know.
2. **Chunking belongs to the caller.** Different clients want different splitting strategies (paragraph-aware, markdown-aware, semantic, sliding-window). Server-side chunking is a real feature that earns its place in a later release.

When content exceeds the limit, the error response includes the actual token count and advises splitting into chunks of <= 7500 tokens (leaving headroom for tokenizer variability).

### Similarity metric

Search uses **cosine similarity** (not cosine distance). pgvector's `<=>` operator returns cosine distance; the server converts it: `similarity = 1.0 - distance`. The result is in [0.0, 1.0] for L2-normalized vectors, where 1.0 means identical and 0.0 means orthogonal.

---

## What the schema does not include

These are deliberately absent in v0.0.1:

- **No `metadata jsonb` column.** Deferred until an actual shape is needed. Tags cover simple labeling.
- **No FTS index.** No full-text search. Semantic search only.
- **No sparse embedding column.** BGE-M3's sparse output exists but is ignored until hybrid search earns its place.
- **No entity or relationship tables.** No knowledge graph.
- **No soft delete or tombstones.** Rows are append-only. No `deleted_at`, no `expires_at`.
- **No access tracking.** No `access_count`, `last_accessed_at`, `importance`, `confidence`.
