# Getting started

## Prerequisites

chitta-rs requires three things on the host machine:

### 1. Postgres with pgvector

Any Postgres 14+ installation with the [pgvector](https://github.com/pgvector/pgvector) extension. Create the database:

```bash
createdb chitta_rs
psql chitta_rs -c 'create extension if not exists vector'
```

Migrations run automatically at startup -- you don't need to apply them manually.

### 2. ONNX Runtime shared library

The `ort` crate loads `libonnxruntime.so` (Linux) or `libonnxruntime.dylib` (macOS) dynamically at startup. Install it via your system package manager:

```bash
# Arch Linux
pacman -S onnxruntime

# Debian / Ubuntu (check distro naming)
apt install libonnxruntime-dev

# macOS (Homebrew)
brew install onnxruntime
```

If the library isn't in a standard location, set `ORT_DYLIB_PATH` to its absolute path:

```bash
export ORT_DYLIB_PATH=/path/to/libonnxruntime.so
```

The Python `onnxruntime` wheel also ships the shared library -- you can point `ORT_DYLIB_PATH` at it if you have a Python environment with onnxruntime installed.

### 3. BGE-M3 model files

chitta-rs uses the [BGE-M3](https://huggingface.co/BAAI/bge-m3) model exported to ONNX format. The default location is `~/.cache/chitta/bge-m3-onnx/`. This directory must contain:

- `bge_m3_model.onnx` -- the ONNX model graph
- `bge_m3_model.onnx_data` -- the external weight sidecar (must be next to the `.onnx` file)
- `tokenizer.json` -- HuggingFace fast-tokenizer format

If you already used the Python chitta, these files are already in place. If not, the [yuniko-software/bge-m3-onnx](https://huggingface.co/yuniko-software/bge-m3-onnx) export is the tested source.

To use a different location, set `CHITTA_MODEL_PATH`:

```bash
export CHITTA_MODEL_PATH=/path/to/model/directory
```

## Installation

Build from source (Rust 2024 edition, requires rustc 1.85+):

```bash
cd rust/
cargo build --release
```

The binary is at `target/release/chitta-rs`.

## Configuration

Copy the example environment file and edit it:

```bash
cp .env.example .env
```

The only required variable is `DATABASE_URL`:

```bash
DATABASE_URL=postgres://localhost/chitta_rs
```

All other variables have sensible defaults. See [Configuration](configuration.md) for the full reference.

## First run (stdio)

Start the server in stdio mode (the default):

```bash
./target/release/chitta-rs
```

chitta-rs reads MCP requests from stdin and writes responses to stdout. Logs go to stderr. On first run it applies database migrations automatically.

You should see a log line on stderr like:

```
INFO starting chitta-rs version=0.0.2 model_path="~/.cache/chitta/bge-m3-onnx"
```

The server is now waiting for MCP messages on stdin.

## First run (HTTP)

To start the server in Streamable HTTP mode for multi-session use:

```bash
# Create a bearer token file
echo "my-secret-token" > /path/to/token.txt

# Start in HTTP mode
./target/release/chitta-rs --http --auth-token-file /path/to/token.txt
```

You should see:

```
INFO chitta-rs HTTP server listening addr=127.0.0.1:3100
```

To bind to a different address or port:

```bash
./target/release/chitta-rs --http --auth-token-file /path/to/token.txt --http-addr 0.0.0.0 --http-port 8080
```

CLI flags override environment variables (`CHITTA_HTTP_ADDR`, `CHITTA_HTTP_PORT`), which override built-in defaults.

## Connecting an MCP client

### Stdio mode

Configure your MCP client to launch the binary. For example, in a Claude Code `settings.json`:

```json
{
  "mcpServers": {
    "chittars": {
      "command": "/path/to/chitta-rs",
      "args": []
    }
  }
}
```

### HTTP mode

Point your MCP client at the HTTP endpoint with bearer auth:

```json
{
  "mcpServers": {
    "chittars": {
      "type": "http",
      "url": "http://localhost:3100/mcp",
      "headers": {
        "Authorization": "Bearer my-secret-token"
      }
    }
  }
}
```

The server announces itself with instructions and seven tools: `store_memory`, `get_memory`, `search_memories`, `update_memory`, `delete_memory`, `list_recent_memories`, `health_check`.

## Replay subcommand

v0.0.2 includes a `replay` subcommand for retrieval regression detection. It re-runs logged search queries against the current database state and reports overlap:

```bash
./target/release/chitta-rs replay --limit 50
./target/release/chitta-rs replay --profile my-profile --limit 100
```

This requires query logging to be enabled (it is by default) and for search queries to have been logged previously.

## Running tests

chitta-rs has three test tiers:

### Unit tests (no external dependencies)

```bash
cargo test --lib
```

These test validation rules, error contract shapes, envelope serialization, and config parsing. They run everywhere with no setup.

### Contract tests (no external dependencies)

```bash
cargo test --test contract
```

These lock the wire contract: argument deserialization shapes, output serialization keys, error-to-JSON-RPC mapping. They catch field renames or type changes before any integration test or client notices.

### Integration tests (require Postgres + model files)

```bash
createdb chitta_rs_test
export TEST_DATABASE_URL=postgres://localhost/chitta_rs_test
cargo test --test integration
```

These exercise the full pipeline: store, get, search, update, delete, list, idempotency, profile isolation, semantic similarity, budget truncation, tag filtering, min_similarity, error contracts, and concurrent writes. They use a separate test database and create unique profiles per test to avoid interference.

If `TEST_DATABASE_URL` is unset or the model files are missing, integration tests skip cleanly (print `SKIPPED:` and pass), so `cargo test` always succeeds even without infrastructure.

## Stopping the server

Send SIGINT (Ctrl-C) or SIGTERM. The server shuts down gracefully. In HTTP mode, active connections are drained before exit.
