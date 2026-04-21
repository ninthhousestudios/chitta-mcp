# Architecture

## Overview

chitta-rs is a single async Rust binary that speaks MCP over stdio or Streamable HTTP. It embeds text locally using BGE-M3 via ONNX Runtime, stores memories in Postgres with pgvector, and retrieves them by semantic similarity.

```
MCP client (stdio or HTTP)
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
pool
```

## Module structure

The crate is a combined library + binary. The library (`src/lib.rs`) exposes all modules so integration tests can drive tool handlers directly without subprocess overhead.

```
src/
  lib.rs          re-exports all modules
  main.rs         binary entrypoint: config, logging, transport selection, server lifecycle
  config.rs       environment-driven configuration (Config::from_env)
  db.rs           Postgres operations: connect, migrate, insert, get, update, delete, search, list, query_log
  embedding.rs    BGE-M3 via ONNX Runtime: tokenize, validate, embed (pooled sessions)
  envelope.rs     response envelope type + token estimator
  error.rs        ChittaError enum, ErrorData struct, error-to-wire mapping
  mcp.rs          rmcp server: ChittaServer, tool wiring, error translation
  tools/
    mod.rs        re-exports Args/Output types
    store.rs      store_memory handler
    get.rs        get_memory handler
    search.rs     search_memories handler
    update.rs     update_memory handler
    delete.rs     delete_memory handler
    list.rs       list_recent_memories handler
    health.rs     health_check handler
    validate.rs   argument validation functions

migrations/
  0001_init.sql       initial schema (memories table + indexes)
  0002_query_log.sql  query log table for retrieval research

tests/
  contract.rs     L0: wire shape tests (no DB, no subprocess)
  integration.rs  L2: full pipeline against live Postgres + ONNX
```

## Startup sequence

`main.rs` drives startup in a fixed order:

1. **Load .env** -- best-effort via `dotenvy`. Missing `.env` is fine.
2. **Parse CLI** -- `clap`-derived `Cli` struct. Supports `--http` flag, HTTP address/port overrides, `--auth-token-file`, and the `replay` subcommand.
3. **Route subcommand** -- if `replay` was requested, run it and exit. Otherwise continue to server startup.
4. **Load config** -- `Config::from_env()` reads environment variables. `DATABASE_URL` is required; everything else has defaults. Unparseable numeric values print a warning and fall back to defaults.
5. **Initialize logging** -- `tracing-subscriber` writes to stderr with the configured filter level. Stdout stays clean for MCP (stdio mode).
6. **Connect to Postgres** -- `db::connect()` creates a connection pool with configurable limits.
7. **Run migrations** -- `db::run_migrations()` applies migrations idempotently via sqlx's migration runner.
8. **Probe query_log** -- check if the `query_log` table exists. If the table is missing, warn and disable logging. If the probe fails for a transient DB reason, abort startup rather than run with ambiguous state.
9. **Load embedder** -- `Embedder::load()` creates a pool of ONNX sessions (default 1, configurable via `CHITTA_EMBEDDER_POOL_SIZE`) and the tokenizer. Each session takes ~1-2 seconds to load.
10. **Start transport** -- either `serve_stdio()` or `serve_http()` depending on the `--http` flag.
11. **Wait for shutdown** -- `tokio::select!` waits on either the MCP service completing or a shutdown signal (SIGINT/SIGTERM on Unix, Ctrl-C elsewhere). HTTP mode drains connections before exit.

If any step fails, the process exits with a descriptive error. There is no retry logic at startup -- a failed connection or missing model is a fatal configuration problem.

## Request lifecycle

When an MCP client sends a tool call:

1. **rmcp** deserializes the JSON-RPC request and routes it to the matching handler on `ChittaServer` via the `ToolRouter`.
2. **The handler** (e.g., `store_memory`) receives typed `Args` -- rmcp deserializes the parameters using `serde` and validates the JSON schema via `schemars`. The schema is published to the client at connection time.
3. **Validation** -- the handler calls `validate::*` functions for each argument. These are pure functions that return `ChittaError::InvalidArgument` with full context on failure.
4. **Embedding** (store, update, and search) -- the handler calls `Embedder::embed()`, which acquires a session from the pool and dispatches inference to `tokio::task::spawn_blocking`.
5. **Database operation** -- the handler calls the appropriate `db::*` function against the connection pool.
6. **Response** -- the handler builds a typed output struct, serializes it to JSON, and returns it to rmcp, which frames it as a JSON-RPC response.

If any step fails, the `ChittaError` is translated to an rmcp `ErrorData` via `chitta_to_rmcp()`, which maps it to the appropriate JSON-RPC error code and serializes the `data` payload.

## Concurrency model

v0.0.2 supports concurrent requests via the HTTP transport. The concurrency model is designed around this:

- **Embedder pool.** The ONNX sessions are behind a pool of `std::sync::Mutex`-wrapped sessions, gated by a `tokio::sync::Semaphore`. The semaphore limits concurrent embeddings to `pool_size`; `acquire_session()` does round-robin `try_lock` to find an available session. Each session runs inside `catch_unwind` -- if ONNX panics, the session is replaced rather than poisoning the server.
- **Connection pool.** sqlx's `PgPool` handles connection management, reconnection, idle timeout, and health checks. Default `max_connections=8`.
- **spawn_blocking.** Embedding inference is dispatched to tokio's blocking thread pool so it never blocks the async runtime. This is critical for HTTP mode where multiple requests may be in flight.
- **Query log.** Search queries are logged via fire-and-forget `tokio::spawn` tasks. Log failures are traced at warn level but never block the search response.

## Shared state

`ChittaServer` holds three pieces of shared state:

- `PgPool` -- sqlx's connection pool. Clone-cheap (internally `Arc`-wrapped). Shared across all requests.
- `Arc<Embedder>` -- the ONNX session pool + tokenizer. Created once at startup. The `Embedder` struct is `Send + Sync` (sessions are behind `Mutex`), so it moves freely between threads.
- `query_log_enabled: bool` -- whether query logging is active, determined at startup.

Both pool and embedder are passed into `ChittaServer::new()` and cloned per-request as needed. There is no other mutable server state -- no session tracking, no caches, no in-memory indexes.

## Type flow

The type system enforces consistency from wire to database:

```
Wire JSON  --(serde/schemars)-->  StoreArgs  --(validate)-->  MemoryRow  --(sqlx)-->  Postgres
                                                                  |
Wire JSON  <--(serde)--  StoreOutput  <--(row_to_output)----------+
```

- **Args structs** (`StoreArgs`, `GetArgs`, `SearchArgs`, `UpdateArgs`, `DeleteArgs`, `ListArgs`, `HealthArgs`) derive `Deserialize` + `JsonSchema`. The `JsonSchema` derivation means rmcp publishes the input schema on the wire from a single source of truth -- no mirror structs, no field-drift risk.
- **Output structs** (`StoreOutput`, `GetOutput`, `SearchHit`, `UpdateOutput`, `DeleteOutput`, `ListOutput`, `HealthOutput`) derive `Serialize`. They are the canonical wire shape.
- **`MemoryRow`** derives `FromRow` for sqlx. It mirrors the database schema exactly.
- **`Envelope<T>`** wraps retrieval results with `truncated`, `total_available`, and `budget_spent_tokens`.

There are no intermediate DTOs or mapping layers beyond `row_to_output()`, which is a trivial field copy.

## HTTP transport

When started with `--http`, chitta-rs runs a Streamable HTTP server (MCP 2025-11-05 spec) via rmcp's `StreamableHttpService`:

- **Endpoint:** `POST /mcp`
- **Auth:** Bearer token via `Authorization` header, validated by `tower-http`'s `ValidateRequestHeaderLayer`. The token is read from a file specified by `--auth-token-file` (required in HTTP mode).
- **Session management:** `LocalSessionManager` manages MCP sessions. Each client gets its own `ChittaServer` instance (cheap -- shares the same pool and embedder Arc).
- **Shutdown:** `CancellationToken` triggers connection draining on SIGINT/SIGTERM before the process exits.
