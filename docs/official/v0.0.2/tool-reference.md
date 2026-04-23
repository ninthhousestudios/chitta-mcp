# Tool reference

chitta-rs exposes seven MCP tools. All tool calls except `health_check` require a `profile` argument -- there is no implicit current profile.

## store_memory

Store a new memory. Idempotent on `(profile, idempotency_key)`: resubmitting the same key returns the prior row without creating a new one.

### Arguments

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `profile` | string | yes | -- | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `content` | string | yes | -- | Non-empty, <= 4 MB bytes, tokenized length <= 8192 tokens |
| `idempotency_key` | string | yes | -- | 1-128 chars, no control characters |
| `event_time` | string (ISO-8601) | no | `record_time` | >= 1970-01-01T00:00:00Z, <= now + 365 days |
| `tags` | array of strings | no | `[]` | Max 32 tags, each 1-64 chars |

### Response

```json
{
  "id": "019713a4-8f2c-7def-b123-456789abcdef",
  "profile": "default",
  "content": "Postgres connection pooling best practices under heavy load",
  "event_time": "2026-04-19T10:12:00Z",
  "record_time": "2026-04-19T10:12:00.123Z",
  "tags": ["db", "perf"],
  "idempotent_replay": false
}
```

| Field | Type | Description |
|---|---|---|
| `id` | string (UUID v7) | Time-sortable unique identifier, generated server-side |
| `profile` | string | The profile this memory belongs to |
| `content` | string | Verbatim stored text, identical to input |
| `event_time` | string (ISO-8601) | When the subject happened. Equals `record_time` if not supplied |
| `record_time` | string (ISO-8601) | When chitta-rs received the memory. Server-set, never changes |
| `tags` | array of strings | The tags stored with this memory |
| `idempotent_replay` | boolean | `true` if this was a duplicate write -- the returned row is the pre-existing one |

### Write path

1. Validate all arguments (profile format, content non-empty, byte-length cap, idempotency_key format, event_time bounds, tag constraints).
2. Check for idempotent replay via `(profile, idempotency_key)` lookup. If found, return the existing row.
3. Embed the content via BGE-M3 (rejects if > 8192 tokens -- see [Data model: content length policy](data-model.md#content-length-policy)).
4. Build a `MemoryRow` with a new UUID v7 and `record_time = now()`.
5. Insert into Postgres. On `(profile, idempotency_key)` conflict (unique constraint violation), fetch the existing row instead.
6. Return the row with `idempotent_replay` indicating whether this was a new insert or a replay.

### Idempotency contract

The unique index on `(profile, idempotency_key)` is the dedup mechanism. The write path does a pre-flight SELECT to skip embedding on replays, then falls back to Postgres error code `23505` (unique violation) for concurrent races. This means:

- First write: inserts the row, returns it with `idempotent_replay: false`.
- Subsequent writes with the same key: return the original row with `idempotent_replay: true`. No new embedding is computed, no new row is created.
- Concurrent writes with the same key: both succeed, both return the same row. Exactly one row exists in the database. At least one of the two callers sees `idempotent_replay: true`.

The idempotency key is scoped to a profile. The same key in different profiles creates separate memories.

---

## get_memory

Fetch a single memory by profile and ID. Returns the full verbatim content -- this is how you read the body after finding a memory via search or list.

### Arguments

| Name | Type | Required | Constraints |
|---|---|---|---|
| `profile` | string | yes | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `id` | string (UUID) | yes | Valid UUID |

### Response

```json
{
  "id": "019713a4-8f2c-7def-b123-456789abcdef",
  "profile": "default",
  "content": "Postgres connection pooling best practices under heavy load",
  "event_time": "2026-04-19T10:12:00Z",
  "record_time": "2026-04-19T10:12:00.123Z",
  "tags": ["db", "perf"]
}
```

Same shape as `store_memory` output, minus `idempotent_replay`.

### Errors

If no memory matches `(profile, id)`, returns a `not_found` error with `next_action` suggesting the caller verify the profile and ID or use `search_memories` to locate the intended memory.

The lookup is profile-scoped: a memory stored under profile `A` is not visible to a `get_memory` call under profile `B`, even with the correct ID.

---

## search_memories

Semantic similarity search. Returns ranked results inside a token-budgeted envelope. Results carry 200-character snippets, not full content -- call `get_memory(id)` to read the body.

### Arguments

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `profile` | string | yes | -- | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `query` | string | yes | -- | Non-empty, <= 4 MB bytes |
| `k` | integer | no | 10 | Range: [1, 200] |
| `max_tokens` | integer | no | unbounded | Must be > 0 if set |
| `tags` | array of strings | no | no filter | Same constraints as store_memory tags |
| `min_similarity` | float | no | 0.0 | Range: [0.0, 1.0], must be finite |

### Response

The response is an envelope wrapping search hits:

```json
{
  "results": [
    {
      "id": "019713a4-8f2c-7def-b123-456789abcdef",
      "snippet": "Postgres connection pooling best pra...",
      "similarity": 0.87,
      "event_time": "2026-04-19T10:12:00Z",
      "record_time": "2026-04-19T10:12:00.123Z",
      "tags": ["db", "perf"]
    }
  ],
  "truncated": false,
  "total_available": 42,
  "budget_spent_tokens": 85
}
```

#### Envelope fields

| Field | Type | Description |
|---|---|---|
| `results` | array | Ranked search hits, most similar first |
| `truncated` | boolean | `true` if `k`, `max_tokens`, or `min_similarity` cut the list short |
| `total_available` | integer or null | Count of rows matching profile + tag filter (ignores `min_similarity` -- see below) |
| `budget_spent_tokens` | integer | Approximate token cost of the entire response. Estimated as `ceil(json_bytes / 4)` |

#### Hit fields

| Field | Type | Description |
|---|---|---|
| `id` | string (UUID) | Memory ID. Pass to `get_memory` for full content |
| `snippet` | string | First 200 characters of content, verbatim. No ellipsis. If content is <= 200 chars, snippet equals content |
| `similarity` | float | Cosine similarity in [0.0, 1.0]. Higher is more similar |
| `event_time` | string (ISO-8601) | When the subject happened |
| `record_time` | string (ISO-8601) | When chitta-rs stored it |
| `tags` | array of strings | Tags on this memory |

### Search behavior

**Embedding.** The query string is embedded using the same BGE-M3 model as stored content. The resulting vector is compared against stored embeddings using cosine similarity via pgvector's HNSW index.

**Tag filtering.** When `tags` is provided, a memory matches if it shares at least one tag with the query (OR semantics). When `tags` is omitted or empty, no tag filter is applied.

**Similarity floor.** When `min_similarity` is set, results below that cosine similarity threshold are excluded. This is a post-HNSW filter -- the ANN index still drives ordering.

**Token budget.** When `max_tokens` is set, results are appended to the response until the next result would push `budget_spent_tokens` over the cap. The budget accounts for the envelope wrapper overhead, not just hit payloads. The first result is always included even if it alone exceeds the cap -- an empty envelope is less useful than a slightly oversize first result.

**Query logging.** When query logging is enabled, each search call logs the query text, embedding vector, parameters (k, min_similarity, tags), result IDs with scores, and latency to the `query_log` table. This is fire-and-forget -- log failures never block the search response.

### Truncation semantics

`truncated` is `true` when any of these conditions hold:

- The `k` limit cut off results (the database returned `k` rows but `total_available` is larger).
- The `max_tokens` budget stopped accumulation before all matching results were included.

`total_available` is the count of rows matching `profile` + `tags` filter. It deliberately **ignores `min_similarity`**. Counting rows above a cosine threshold would require scanning every embedding, defeating the ANN index. The caller gets a truthful ceiling on how many memories exist in the scope; the actual results show the similarity-gated subset.

### HNSW tuning

To ensure the ANN index returns enough candidates for post-filtering (tags, min_similarity), chitta-rs raises pgvector's `hnsw.ef_search` parameter per query. The value is `clamp(k * 4, 200, 1000)`. This is set via `SET LOCAL` inside a transaction, so it never leaks to other pool connections.

---

## update_memory

Update a memory's content and/or tags. At least one of `content` or `tags` must be provided. If content changes, the embedding is recomputed. `record_time` is never modified (bi-temporal invariant).

### Arguments

| Name | Type | Required | Constraints |
|---|---|---|---|
| `profile` | string | yes | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `id` | string (UUID) | yes | Valid UUID |
| `content` | string | no | Non-empty if provided, <= 4 MB bytes, tokenized length <= 8192 tokens |
| `tags` | array of strings | no | Max 32 tags, each 1-64 chars |

At least one of `content` or `tags` must be provided.

### Response

```json
{
  "id": "019713a4-8f2c-7def-b123-456789abcdef",
  "profile": "default",
  "content": "Updated content text",
  "event_time": "2026-04-19T10:12:00Z",
  "record_time": "2026-04-19T10:12:00.123Z",
  "tags": ["db", "perf", "new-tag"],
  "re_embedded": true
}
```

| Field | Type | Description |
|---|---|---|
| `id` | string (UUID v7) | The memory's ID (unchanged) |
| `profile` | string | The profile (unchanged) |
| `content` | string | The new content (or unchanged content if only tags were updated) |
| `event_time` | string (ISO-8601) | Original event_time (unchanged) |
| `record_time` | string (ISO-8601) | Original record_time (unchanged -- bi-temporal invariant) |
| `tags` | array of strings | The new tags (or unchanged tags if only content was updated) |
| `re_embedded` | boolean | `true` if content changed and the embedding was recomputed |

### Update behavior

The update is atomic -- it uses `UPDATE ... SET ... WHERE profile = $1 AND id = $2 RETURNING *` with `COALESCE` so only provided fields are overwritten. If the `(profile, id)` pair doesn't exist, the tool returns a `not_found` error.

### Errors

- **not_found** if the `(profile, id)` pair doesn't exist.
- **invalid_argument** if neither `content` nor `tags` is provided.
- **content_too_long** if updated content exceeds the 8192-token limit.

---

## delete_memory

Hard-delete a memory by profile + ID. The row is permanently removed -- there is no soft delete, no undo.

### Arguments

| Name | Type | Required | Constraints |
|---|---|---|---|
| `profile` | string | yes | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `id` | string (UUID) | yes | Valid UUID |

### Response

```json
{
  "id": "019713a4-8f2c-7def-b123-456789abcdef",
  "deleted": true
}
```

| Field | Type | Description |
|---|---|---|
| `id` | string (UUID) | The deleted memory's ID |
| `deleted` | boolean | Always `true` on success |

### Errors

If no memory matches `(profile, id)`, returns a `not_found` error.

---

## list_recent_memories

List memories ordered by `record_time DESC` (most recent first). Returns 200-character snippets -- call `get_memory(id)` for full content. Useful for "what happened recently" queries that semantic search doesn't cover.

### Arguments

| Name | Type | Required | Default | Constraints |
|---|---|---|---|---|
| `profile` | string | yes | -- | 1-128 chars, `[a-zA-Z0-9_-]+` only |
| `limit` | integer | no | 20 | Range: [1, 200] |
| `tags` | array of strings | no | no filter | Same constraints as store_memory tags |

### Response

```json
{
  "memories": [
    {
      "id": "019713a4-8f2c-7def-b123-456789abcdef",
      "snippet": "Postgres connection pooling best pra...",
      "event_time": "2026-04-19T10:12:00Z",
      "record_time": "2026-04-19T10:12:00.123Z",
      "tags": ["db", "perf"]
    }
  ],
  "total_in_profile": 42
}
```

| Field | Type | Description |
|---|---|---|
| `memories` | array | Recent memories, most recent first |
| `total_in_profile` | integer | Total count of all memories in the profile (regardless of tag filter) |

Each memory item has the same shape as a search hit minus `similarity`.

### Tag filtering

When `tags` is provided, only memories sharing at least one tag are returned (OR semantics). `total_in_profile` always reflects the full profile count regardless of tag filter.

The list query and count run inside a single transaction for MVCC consistency.

---

## health_check

Verify server health. Checks DB connectivity and embedder responsiveness. Designed for agent startup probes -- call this at session start to confirm the server is operational before issuing memory operations.

### Arguments

None. Pass an empty object `{}`.

### Response

```json
{
  "status": "ok",
  "db_connected": true,
  "embedder_ok": true,
  "embedder_pool_size": 1,
  "version": "0.0.2"
}
```

| Field | Type | Description |
|---|---|---|
| `status` | string | `"ok"` if all checks pass, `"degraded"` if any component is unhealthy |
| `db_connected` | boolean | `true` if a `SELECT 1` against the database succeeded |
| `embedder_ok` | boolean | `true` if a trivial embedding completed successfully |
| `embedder_pool_size` | integer | Number of ONNX sessions in the embedder pool |
| `version` | string | Server version from Cargo.toml |

### Behavior

The health check runs a lightweight DB ping (`SELECT 1`) and a trivial embedding probe. Neither operation modifies any state. If either check fails, the corresponding flag is `false` and `status` is `"degraded"` rather than `"ok"`.

This tool never returns an error -- it always succeeds with a status report. A `"degraded"` status tells the agent to expect failures on subsequent tool calls and to include that context in any error reports.
