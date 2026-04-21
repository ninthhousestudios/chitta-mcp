# chitta-rs v0.0.2

chitta-rs is a self-hosted, agent-native persistent memory server. It lets AI agents store, retrieve, update, delete, and search memories over the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP). This is the Rust rewrite of the original Python chitta, designed from scratch around a small set of [foundational principles](../principles.md).

## What it does

An agent connects to chitta-rs over stdio or HTTP and gets seven tools:

- **store_memory** -- persist a piece of text with bi-temporal timestamps, tags, and idempotent write semantics.
- **get_memory** -- fetch a memory by ID, returning the full verbatim content.
- **search_memories** -- semantic similarity search using BGE-M3 embeddings, returning ranked snippets inside a token-budgeted envelope.
- **update_memory** -- update a memory's content and/or tags by profile + ID. Re-embeds if content changes.
- **delete_memory** -- hard-delete a memory by profile + ID.
- **list_recent_memories** -- time-ordered retrieval (most recent first) with optional tag filter.
- **health_check** -- verify DB connectivity and embedder responsiveness. Returns server status, component health flags, pool size, and version.

Seven tools, one table, local embeddings, Postgres storage, two transports.

## What changed from v0.0.1

- **Four new tools:** update_memory, delete_memory, list_recent_memories, health_check. Memories are no longer append-only.
- **Streamable HTTP transport.** `--http` flag starts an HTTP server with bearer-token auth. Multiple Claude Code sessions can share one chitta-rs process.
- **Embedder session pool.** Configurable pool of ONNX sessions (`CHITTA_EMBEDDER_POOL_SIZE`) with `catch_unwind` panic recovery and automatic session replacement. Concurrent embedding requests run in parallel.
- **Query logging.** Every `search_memories` call logs query text, embedding, parameters, result IDs, scores, and latency to a `query_log` table. The `replay` subcommand re-runs logged queries for retrieval regression detection.
- **Search query validation.** Search queries now have the same 4MB byte-length cap as stored content.
- **Config warnings.** Unparseable environment variable values now print a warning instead of silently falling back to defaults.

## What it is not

v0.0.2 deliberately excludes everything the Python chitta accumulated that didn't earn its place through benchmarks or documented need: entity extraction, knowledge graphs, language packs, FSRS scheduling, PageRank, hybrid search, and 35 other tools. Those re-enter through the release-gate process, not through porting.

## Documentation contents

| Document | What it covers |
|---|---|
| [Getting started](getting-started.md) | Prerequisites, installation, first run (stdio and HTTP) |
| [Architecture](architecture.md) | Module structure, data flow, concurrency model |
| [Tool reference](tool-reference.md) | Complete API for all seven tools |
| [Data model](data-model.md) | Database schema, embedding pipeline, temporal model |
| [Error handling](errors.md) | Error contract, error types, recovery guidance |
| [Configuration](configuration.md) | Environment variables, CLI flags, defaults, pool tuning |

## Key design decisions

**Verbatim storage.** Content is stored exactly as received. Embeddings, snippets, and any future derived data are always re-derivable from the source text. The server never rewrites, summarizes, or truncates stored content.

**Bi-temporal timestamps.** Every memory carries two times: `event_time` (when the thing happened in the world) and `record_time` (when chitta-rs learned about it). This distinction matters for temporal reasoning -- an agent can record a fact today about something that happened last week.

**Idempotent writes.** Every `store_memory` call requires a client-supplied `idempotency_key`. The same `(profile, idempotency_key)` pair always returns the same row. Retries are safe. Concurrent duplicate writes converge to one row.

**Profile isolation.** Every tool call requires a `profile` argument. Profiles are the only namespace mechanism. There is no server-side session state, no implicit current profile. This makes the same binary usable for single-user and multi-tenant deployments without architectural changes.

**Actionable errors.** Every error tells the caller what tool failed, what constraint was violated, and what to try next. No stack traces leave the server. No `Error: invalid input` without guidance.

**Local embeddings.** Embedding runs locally via ONNX Runtime and the BGE-M3 model. No API calls to external services. No network dependency on the embedding path. The model files live on disk and are loaded once at startup.
