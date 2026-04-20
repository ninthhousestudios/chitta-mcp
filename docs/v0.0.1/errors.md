# Error handling

Every error chitta-rs returns is designed to be actionable. The server never sends bare error messages -- every error carries structured context that tells the caller what went wrong and what to do about it.

## Wire format

All errors use JSON-RPC 2.0 error format with a populated `data` field:

```json
{
  "code": -32602,
  "message": "invalid argument `event_time` for tool `store_memory`: ISO-8601 timestamp >= 1970-01-01T00:00:00Z",
  "data": {
    "tool": "store_memory",
    "argument": "event_time",
    "constraint": "ISO-8601 timestamp >= 1970-01-01T00:00:00Z",
    "received": "1969-06-20T00:00:00Z",
    "next_action": "Pass event_time >= 1970-01-01T00:00:00Z, or omit to default to record_time."
  }
}
```

## Error data contract

Every error's `data` object carries these fields:

| Field | Always present | Description |
|---|---|---|
| `tool` | yes | Which tool or subsystem produced the error (e.g., `store_memory`, `get_memory`, `database`, `startup`, `server`) |
| `constraint` | yes | What invariant was violated |
| `next_action` | yes | What the caller should try next |
| `argument` | sometimes | Which argument was invalid (present on validation errors) |
| `received` | sometimes | The offending value or diagnostic context (present when useful) |

When `argument` or `received` would be null, they are omitted from the JSON entirely (not serialized as `null`).

## JSON-RPC error codes

chitta-rs uses two standard JSON-RPC error codes:

| Code | Meaning | When used |
|---|---|---|
| `-32602` | Invalid params | Caller-fixable errors: bad arguments, missing config, not-found, content too long |
| `-32603` | Internal error | Server-side faults: database failures, embedding pipeline errors, bugs |

The distinction matters for retry logic: `-32602` errors are deterministic (same input will fail the same way), while `-32603` errors may be transient (retry might succeed).

## Error types

### MissingConfig

A required environment variable is not set. Only raised at startup, not during tool calls.

```json
{
  "code": -32602,
  "data": {
    "tool": "startup",
    "argument": "DATABASE_URL",
    "constraint": "environment variable `DATABASE_URL` must be set",
    "next_action": "Set DATABASE_URL in the environment or .env file (e.g. postgres://localhost/chitta_rs)."
  }
}
```

### InvalidArgument

A tool argument violates its documented constraint. Each validation rule produces a specific error:

| Argument | Constraint | Example `next_action` |
|---|---|---|
| `profile` | 1-128 chars, `[a-zA-Z0-9_-]+` | Pass a non-empty profile of <= 128 ASCII letters, digits, underscores, or hyphens. |
| `content` | length >= 1 | Pass non-empty content. |
| `idempotency_key` | 1-128 chars, no control characters | Pass a 1-128 character idempotency_key with no control characters (e.g. a UUID or a client-stable hash). |
| `event_time` | >= 1970-01-01T00:00:00Z | Pass event_time >= 1970-01-01T00:00:00Z, or omit to default to record_time. |
| `event_time` | <= now + 365 days | Pass event_time within one year of now, or omit to default to record_time. |
| `tags` | at most 32 tags | Trim the tag list to at most 32 entries. |
| `tags` (each) | 1-64 chars per tag | Ensure every tag is between 1 and 64 characters. |
| `k` | integer in [1, 200] | Pass k between 1 and 200 (default is 10). |
| `min_similarity` | finite float in [0.0, 1.0] | Pass min_similarity between 0.0 and 1.0 inclusive. |
| `max_tokens` | > 0 | Pass a positive max_tokens, or omit to disable the budget. |
| `id` | valid UUID | Pass a valid UUID string. |

All `InvalidArgument` errors include `received` with the offending value so the caller can see exactly what was rejected.

### ContentTooLong

The content exceeds BGE-M3's 8192-token context window. This is checked after tokenization, so the count is exact (not an estimate).

```json
{
  "code": -32602,
  "data": {
    "tool": "store_memory",
    "argument": "content",
    "constraint": "tokenized length <= 8192",
    "received": { "token_count": 11432 },
    "next_action": "Split content into chunks of <= 7500 tokens and store each as a separate memory with its own idempotency_key"
  }
}
```

The `next_action` suggests 7500 tokens (not 8192) to give headroom for tokenizer variability across content types.

### NotFound

The requested `(profile, id)` does not match any row. Only raised by `get_memory`.

```json
{
  "code": -32602,
  "data": {
    "tool": "get_memory",
    "constraint": "memory exists in the given profile",
    "next_action": "Verify the profile and id, or call search_memories to locate the intended memory."
  }
}
```

### Embedding

An error in the embedding pipeline: tokenizer failure, ONNX session error, unexpected output shape.

```json
{
  "code": -32603,
  "data": {
    "tool": "store_memory",
    "constraint": "embedding pipeline completes without error",
    "received": { "message": "ONNX runtime error: ..." },
    "next_action": "Report this as a bug; include server logs."
  }
}
```

At startup, embedding errors include more specific guidance (check `CHITTA_MODEL_PATH`, ensure `libonnxruntime` is installed).

### Db (database errors)

Errors from the Postgres connection pool or query execution. The `next_action` varies by failure mode:

| Failure type | `next_action` guidance |
|---|---|
| Pool timeout, pool closed, worker crash | Retry. If it repeats, check server load and DATABASE_URL reachability. |
| I/O, TLS, protocol errors | Retry. If it repeats, check database reachability and network configuration. |
| Database rejection (constraint, permission, schema) | Inspect the message, correct the input or schema, and retry. |
| Row not found | Verify the ID; otherwise report as a bug. |
| Column/decode/type errors | Schema drift between migrations and server code. Rebuild. |

### Migrate

Migration failure at startup. The server exits with this error; it does not appear in tool call responses.

### Internal

Catch-all for unexpected server faults (mutex poisoning, serialization failure, spawn_blocking failure). The `next_action` is always "Report this as a bug; include server logs."

## Error translation to JSON-RPC

The `chitta_to_rmcp` function in `mcp.rs` translates every `ChittaError` variant to rmcp's `ErrorData`:

1. Extract the JSON-RPC code from the error variant (`-32602` or `-32603`).
2. Format the human-readable `message` via the `Display` implementation.
3. Serialize the `ErrorData` struct to JSON for the `data` field.
4. Route to `ErrorData::invalid_params()` or `ErrorData::internal_error()` based on the code.

This translation is tested exhaustively in `tests/contract.rs` -- every error variant is walked through the mapper and the output is verified for correct code, non-empty message, and populated `data.tool`, `data.constraint`, and `data.next_action`.

## Design invariants

- **No stack traces on the wire.** Internal error details stay in server logs (stderr). The `data` payload carries enough context for the caller to act without exposing implementation details.
- **Every variant tested.** The `every_variant_populates_contract` test in `error.rs` constructs a representative of every `ChittaError` variant and asserts that `tool`, `constraint`, and `next_action` are non-empty. Adding a new variant without wiring it properly fails the build.
- **`None` fields omitted.** When `argument` or `received` are `None`, they are skipped during serialization (not sent as `null`). This keeps the wire payload clean.
