# chitta-rs

Agent-native persistent memory server — Rust rewrite of chitta, speaking MCP
over stdio. v0.0.1 is a narrow slice: one backend (Postgres + pgvector), one
embedder (BGE-M3 ONNX), three tools (`store_memory`, `get_memory`,
`search_memories`), verbatim storage, bi-temporal rows, idempotent writes,
actionable errors.

See [`docs/principles.md`](docs/principles.md) for the contract this server
upholds and [`docs/starting-shape.md`](docs/starting-shape.md) for the wire
shape and design rationale.

## Prerequisites

- **Rust** stable (edition 2024 — 1.85+).
- **Postgres 16+** with the [`pgvector`](https://github.com/pgvector/pgvector)
  extension installed.
- **BGE-M3 ONNX model** at `~/.cache/chitta/bge-m3-onnx/` containing
  `bge_m3_model.onnx` (plus its `.onnx_data` sidecar if the export is
  external-data) and `tokenizer.json`. We use the
  [`yuniko-software/bge-m3-onnx`](https://huggingface.co/yuniko-software/bge-m3-onnx)
  export, which emits a pre-pooled, L2-normalized `dense_embeddings` tensor.
- **ONNX Runtime shared library.** Either install `onnxruntime` via your
  package manager or reuse the copy shipped with Python's `onnxruntime`
  wheel; point `ORT_DYLIB_PATH` at it if it's not on the default loader path.

## Quickstart

```bash
createdb chitta_rs
cp .env.example .env          # edit DATABASE_URL if needed
cargo run                     # starts the MCP server on stdio
```

The binary is a stdio-transport MCP server. It reads JSON-RPC frames from
stdin and writes responses to stdout; logs go to stderr.

## Connecting from an MCP client

Point your client's server entry at the compiled binary. For Claude Code's
`settings.json`:

```json
{
  "mcpServers": {
    "chitta-rs": {
      "command": "/path/to/chitta/rust/target/release/chitta-rs",
      "env": {
        "DATABASE_URL": "postgres://localhost/chitta_rs",
        "CHITTA_MODEL_PATH": "/home/you/.cache/chitta/bge-m3-onnx"
      }
    }
  }
}
```

`cargo build --release` first, then restart the client.

## Configuration

| Variable | Default | Notes |
|---|---|---|
| `DATABASE_URL` | *(required)* | libpq-compatible Postgres URL. |
| `CHITTA_MODEL_PATH` | `~/.cache/chitta/bge-m3-onnx` | Directory with the ONNX model + tokenizer. |
| `CHITTA_LOG_LEVEL` | `info` | `tracing_subscriber` env filter syntax. |
| `ORT_DYLIB_PATH` | *(loader default)* | Path to `libonnxruntime.so` when it's not on the default path. |

## Tools

| Tool | Purpose | Idempotency |
|---|---|---|
| `store_memory` | Persist verbatim content with optional `event_time` and `tags`. | Keyed on `(profile, idempotency_key)` — same key returns the prior row with `idempotent_replay=true`. |
| `get_memory` | Fetch one memory by profile + UUID. | Read-only. |
| `search_memories` | Cosine-similarity search with tag filter, similarity floor, and token-budget truncation. | Read-only. Returns the standard envelope (`results`, `truncated`, `total_available`, `budget_spent_tokens`). |

## Testing

### Unit + contract tests

No external dependencies. Run:

```bash
cargo test --lib
cargo test --test contract
```

### Integration tests

Require a live Postgres with pgvector and the ONNX model on disk:

```bash
createdb chitta_rs_test
psql chitta_rs_test -c 'create extension if not exists vector'
export TEST_DATABASE_URL=postgres://localhost/chitta_rs_test
# If libonnxruntime is not on the default loader path:
# export ORT_DYLIB_PATH=/path/to/libonnxruntime.so
cargo test --test integration
```

Tests without `TEST_DATABASE_URL` set (or with the model missing) print a
`SKIPPED:` line and pass — so `cargo test` stays useful in environments
without the full stack.

### Lint

```bash
cargo clippy --all-targets -- -D warnings
```

## What's deferred to v0.0.2+

- HTTP transport (`--http` is stubbed and exits with a deferral message).
- Tag-AND search, full-text filter, time-range filter.
- Additional tools (`list_recent_memories`, `delete_memory`, graph walks).
- Automatic migration of Python chitta data.
- Embedder alternatives beyond ONNX BGE-M3.

See [`docs/starting-shape.md`](docs/starting-shape.md) for the full list and
the reasoning behind each deferral.
