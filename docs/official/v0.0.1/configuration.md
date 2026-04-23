# Configuration

All configuration is via environment variables. A `.env` file in the working directory is loaded at startup via `dotenvy` (best-effort -- missing `.env` is fine). There are no config files, no runtime reconfiguration. Restart the server to change settings.

## Environment variables

### Required

| Variable | Purpose |
|---|---|
| `DATABASE_URL` | Postgres connection string. Any libpq-compatible URL works. Example: `postgres://localhost/chitta_rs` |

### Optional

| Variable | Default | Purpose |
|---|---|---|
| `CHITTA_MODEL_PATH` | `~/.cache/chitta/bge-m3-onnx` | Directory containing the BGE-M3 ONNX model and tokenizer files |
| `CHITTA_LOG_LEVEL` | `info` | Log verbosity filter. Uses `tracing-subscriber` env-filter syntax |
| `CHITTA_DB_MAX_CONNECTIONS` | `8` | Maximum connections in the sqlx connection pool |
| `CHITTA_DB_ACQUIRE_TIMEOUT` | `5` | Seconds to wait for a pool connection before timing out |
| `CHITTA_DB_IDLE_TIMEOUT` | `600` | Seconds before an idle connection is closed |
| `ORT_DYLIB_PATH` | (system default) | Absolute path to `libonnxruntime.so` / `.dylib` / `.dll`. Set this if the library isn't in a standard system location |

### Log levels

`CHITTA_LOG_LEVEL` accepts any valid `tracing-subscriber` env-filter string:

| Value | Effect |
|---|---|
| `trace` | Everything, including per-request detail |
| `debug` | Development-level detail |
| `info` | Startup, shutdown, and notable events (default) |
| `warn` | Warnings and errors only |
| `error` | Errors only |
| `info,sqlx=warn` | Info for chitta-rs, suppress sqlx noise |

Logs always go to stderr. Stdout is reserved for the MCP JSON-RPC stream.

## Connection pool tuning

The defaults (`max_connections=8`, `acquire_timeout=5s`, `idle_timeout=600s`) are sized for single-user stdio. Some guidance for different scenarios:

**Single-user stdio (v0.0.1 default).** The defaults are fine. Only one request is in flight at a time, so one active connection is all you need. The pool is there for reconnection and health-check benefits.

**Development / testing.** Keep defaults. Integration tests create per-test pools with the same settings.

**Future HTTP transport (v0.0.2+).** When concurrent requests arrive, raise `CHITTA_DB_MAX_CONNECTIONS` to match expected concurrency. A good starting point is 2x the expected concurrent tool calls. Lower `CHITTA_DB_IDLE_TIMEOUT` if connections are expensive (e.g., cloud-managed Postgres with per-connection cost).

## Model file layout

The directory at `CHITTA_MODEL_PATH` must contain these files:

```
<CHITTA_MODEL_PATH>/
  bge_m3_model.onnx       ONNX model graph
  bge_m3_model.onnx_data   External weight sidecar
  tokenizer.json            HuggingFace fast-tokenizer
```

The `.onnx_data` file must be adjacent to the `.onnx` file -- `ort` resolves it by relative path. Moving only the `.onnx` file without its sidecar will cause a startup failure.

## Example `.env`

```bash
# Required: Postgres connection string
DATABASE_URL=postgres://localhost/chitta_rs

# Optional: model path (defaults to ~/.cache/chitta/bge-m3-onnx)
# CHITTA_MODEL_PATH=/home/you/.cache/chitta/bge-m3-onnx

# Optional: log level (defaults to info)
# CHITTA_LOG_LEVEL=info

# Optional: pool tuning
# CHITTA_DB_MAX_CONNECTIONS=8
# CHITTA_DB_ACQUIRE_TIMEOUT=5
# CHITTA_DB_IDLE_TIMEOUT=600

# Optional: ONNX Runtime path (if not in system library path)
# ORT_DYLIB_PATH=/usr/lib/libonnxruntime.so
```
