# Architecture

## Overview

chitta-rs is a single async Rust binary that speaks MCP over stdio. It embeds text locally using BGE-M3 via ONNX Runtime, stores memories in Postgres with pgvector, and retrieves them by semantic similarity.

```
MCP client (stdin/stdout)
        |
        v
   +-----------+
   |  rmcp     |  JSON-RPC framing, tool routing
   +-----------+
        |
        v
   +-----------+
   | ChittaServer |  MCP handler: dispatches to tool handlers
   +-----------+
        |
   +----+----+
   |         |
   v         v
Embedder   PgPool
(ONNX)     (sqlx)
```

## Module structure

The crate is a combined library + binary. The library (`src/lib.rs`) exposes all modules so integration tests can drive tool handlers directly without subprocess overhead.

```
src/
  lib.rs          re-exports all modules
  main.rs         binary entrypoint: config, logging, server lifecycle
  config.rs       environment-driven configuration (Config::from_env)
  db.rs           Postgres operations: connect, migrate, insert, get, search
  embedding.rs    BGE-M3 via ONNX Runtime: tokenize, validate, embed
  envelope.rs     response envelope type + token estimator
  error.rs        ChittaError enum, ErrorData struct, error-to-wire mapping
  mcp.rs          rmcp server: ChittaServer, tool wiring, error translation
  tools/
    mod.rs        re-exports Args/Output types
    store.rs      store_memory handler
    get.rs        get_memory handler
    search.rs     search_memories handler
    validate.rs   argument validation functions

migrations/
  0001_init.sql   initial schema (memories table + indexes)

tests/
  contract.rs     L0: wire shape tests (no DB, no subprocess)
  integration.rs  L2: full pipeline against live Postgres + ONNX
```

## Startup sequence

`main.rs` drives startup in a fixed order:

1. **Load .env** -- best-effort via `dotenvy`. Missing `.env` is fine.
2. **Parse CLI** -- `clap`-derived `Cli` struct. The `--http` flag is reserved for v0.0.2 and exits cleanly with a message.
3. **Load config** -- `Config::from_env()` reads environment variables. `DATABASE_URL` is required; everything else has defaults.
4. **Initialize logging** -- `tracing-subscriber` writes to stderr with the configured filter level. Stdout stays clean for MCP.
5. **Connect to Postgres** -- `db::connect()` creates a connection pool with configurable limits.
6. **Run migrations** -- `db::run_migrations()` applies `migrations/0001_init.sql` idempotently via sqlx's migration runner.
7. **Load embedder** -- `Embedder::load()` creates the ONNX session and tokenizer. This takes ~1-2 seconds.
8. **Start MCP server** -- `ChittaServer::new()` wires the tool router, then `.serve((stdin, stdout))` begins the MCP session.
9. **Wait for shutdown** -- `tokio::select!` waits on either the MCP service completing or a shutdown signal (SIGINT/SIGTERM on Unix, Ctrl-C elsewhere).

If any step fails, the process exits with a descriptive error. There is no retry logic at startup -- a failed connection or missing model is a fatal configuration problem.

## Request lifecycle

When an MCP client sends a tool call:

1. **rmcp** deserializes the JSON-RPC request and routes it to the matching handler on `ChittaServer` via the `ToolRouter`.
2. **The handler** (e.g., `store_memory`) receives typed `Args` -- rmcp deserializes the parameters using `serde` and validates the JSON schema via `schemars`. The schema is published to the client at connection time.
3. **Validation** -- the handler calls `validate::*` functions for each argument. These are pure functions that return `ChittaError::InvalidArgument` with full context on failure.
4. **Embedding** (store and search only) -- the handler calls `Embedder::embed()` inside `tokio::task::spawn_blocking` to avoid blocking the async runtime during CPU-bound inference.
5. **Database operation** -- the handler calls the appropriate `db::*` function against the connection pool.
6. **Response** -- the handler builds a typed output struct, serializes it to JSON, and returns it to rmcp, which frames it as a JSON-RPC response.

If any step fails, the `ChittaError` is translated to an rmcp `ErrorData` via `chitta_to_rmcp()`, which maps it to the appropriate JSON-RPC error code and serializes the `data` payload.

## Concurrency model

v0.0.1 uses stdio transport, which is inherently single-request: one MCP message in flight at a time. This simplifies several design points:

- **Embedder mutex.** The ONNX session is behind a `std::sync::Mutex` because `Session::run` requires `&mut self` in ort 2.0.0-rc.10. This is not a bottleneck in v0.0.1 because only one embed call happens at a time. The mutex becomes a concern when HTTP transport arrives in v0.0.2 -- the code documents this explicitly with a remediation plan (embedder thread pool, session pool, or a newer ort version).
- **Connection pool.** sqlx's `PgPool` is still used (not a single connection) because the pool handles reconnection, idle timeout, and health checks transparently. The default `max_connections=8` is generous for stdio; it's there for the HTTP transition.
- **spawn_blocking.** Embedding is dispatched to the blocking thread pool so a hypothetical concurrent request wouldn't be blocked. This is forward-looking for HTTP but costs nothing now.

## Shared state

`ChittaServer` holds two pieces of shared state:

- `PgPool` -- sqlx's connection pool. Clone-cheap (internally `Arc`-wrapped). Shared across all requests.
- `Arc<Embedder>` -- the ONNX session + tokenizer. Created once at startup. The `Embedder` struct is `Send + Sync` (the session is behind a `Mutex`), so it moves freely between threads.

Both are passed into `ChittaServer::new()` and cloned per-request as needed. There is no other mutable server state -- no session tracking, no caches, no in-memory indexes.

## Type flow

The type system enforces consistency from wire to database:

```
Wire JSON  --(serde/schemars)-->  StoreArgs  --(validate)-->  MemoryRow  --(sqlx)-->  Postgres
                                                                  |
Wire JSON  <--(serde)--  StoreOutput  <--(row_to_output)----------+
```

- **Args structs** (`StoreArgs`, `GetArgs`, `SearchArgs`) derive `Deserialize` + `JsonSchema`. The `JsonSchema` derivation means rmcp publishes the input schema on the wire from a single source of truth -- no mirror structs, no field-drift risk.
- **Output structs** (`StoreOutput`, `GetOutput`, `SearchHit`) derive `Serialize`. They are the canonical wire shape.
- **`MemoryRow`** derives `FromRow` for sqlx. It mirrors the database schema exactly.
- **`Envelope<T>`** wraps retrieval results with `truncated`, `total_available`, and `budget_spent_tokens`.

There are no intermediate DTOs or mapping layers beyond `row_to_output()`, which is a trivial field copy.
