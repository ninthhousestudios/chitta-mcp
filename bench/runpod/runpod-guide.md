# RunPod AMB benchmark guide (chitta-rs)

## Pod configuration

- Image: `runpod/pytorch:2.8.0-py3.11-cuda12.8.1-cudnn-devel-ubuntu22.04`
- GPU: L40S or L4 (ONNX embedding runs inside chitta-rs)
- vCPU: 12+
- RAM: 32GB+
- Disk: 80GB+ (WAL bloat from bulk postgres can eat 40-50GB)
- bloat problem fixed. avergae around 27gb during benchs, so can do about 30gb next time

Do NOT use Docker Hub images (rate-limited on RunPod) or Ubuntu 20.04 (ships PG 12, pgvector needs 13+).

## Setup

```bash
export GEMINI_API_KEY="your-key"

git clone https://github.com/ninthhousestudios/agent-memory-benchmark.git /workspace/agent-memory-benchmark
cd /workspace/agent-memory-benchmark

bash bench/runpod/setup.sh
```

Setup installs: postgres + pgvector, Rust toolchain, uv, both repos (chitta + AMB), ONNX runtime, BGE-M3 model. Builds chitta-rs from source.

## Run benchmarks

chitta-rs runs as a background HTTP server. The run scripts handle DB reset, server start, and eval automatically.

### Full evals (LLM-judged, costs Gemini credits)

```bash
# PersonaMem 32k (small, fast, good smoke test)
bash bench/runpod/run-personamem.sh 32k

# Raise k
CHITTA_K=40 bash bench/runpod/run-personamem.sh 32k

# BEAM 100k (runs smoke test first, prompts before full run)
bash bench/runpod/run-beam.sh 100k

# Larger BEAM splits
bash bench/runpod/run-beam.sh 500k
bash bench/runpod/run-beam.sh 1m

# Chunk-size sweep
bash bench/runpod/run-personamem-sweep.sh 32k 50
```

### Retrieval-only evals (zero LLM cost)

These measure retrieval quality (hit rate, recall, MRR) without calling an LLM judge. Fast and free.

```bash
# Dense-only baselines
bash bench/runpod/run-retrieval-personamem.sh 32k
bash bench/runpod/run-retrieval-beam.sh 100k
bash bench/runpod/run-retrieval-lifebench.sh

# Limit queries for a quick spot-check
bash bench/runpod/run-retrieval-beam.sh 100k 50
bash bench/runpod/run-retrieval-lifebench.sh 50
```

### RRF hybrid retrieval sweep

Compares dense-only vs hybrid (FTS, sparse, both) across multiple configs. Resets the DB and restarts chitta-rs for each config so results are independent.

```bash
# Default: personamem 32k
bash bench/runpod/run-retrieval-sweep.sh

# BEAM 100k
bash bench/runpod/run-retrieval-sweep.sh beam 100k

# LifeBench (~3.5 hours for 7-config sweep)
bash bench/runpod/run-retrieval-sweep.sh lifebench en
```

Edit the `CONFIGS` array in the sweep script to add/remove configs. Each entry is `"RUN_NAME:ENV_VAR=VALUE ENV_VAR=VALUE ..."`. RRF-related env vars are unset between iterations so configs don't leak into each other.

Available RRF env vars:

| Var | Default | Description |
|-----|---------|-------------|
| `CHITTA_RRF_FTS` | `false` | Enable FTS leg |
| `CHITTA_RRF_SPARSE` | `false` | Enable sparse re-ranking leg |
| `CHITTA_RRF_K` | `60` | RRF constant (higher = less weight to top ranks) |
| `CHITTA_RRF_CANDIDATES` | `5` | Fetch multiplier per leg (`k * candidates`) |

Results are saved per-config under `outputs/<dataset>/<run-name>/retrieval/<split>.json`, with the full RRF config recorded in each result file.

## Architecture difference from Python chitta

The old setup imported Python chitta in-process. The new setup runs chitta-rs as an HTTP server and the benchmark adapter talks to it via MCP JSON-RPC over HTTP (`http://127.0.0.1:3100/mcp`). This means:

- chitta-rs handles its own ONNX embedding (no Python onnxruntime needed in the AMB venv)
- DB schema is applied automatically via sqlx migrations on chitta-rs startup
- Multiple benchmark processes can share the same chitta-rs server
- The benchmark adapter only needs `httpx` as a dependency

## Known gotchas

1. **Rust build time**: first `cargo build --release` takes ~5-10 min on RunPod. Subsequent builds are cached.
2. **OpenBLAS thread bomb**: defaults to 64 threads, kills limited-vCPU pods. `OPENBLAS_NUM_THREADS=4` must be set before scipy/numpy import.
3. **Postgres auth**: default is peer auth over unix sockets. Scripts use `ALTER USER` + password in DATABASE_URL.
4. **WAL bloat**: bulk ingest bloats WAL. Scripts set `max_wal_size = '2GB'` and run `VACUUM FULL` after.
5. **DB reset requires chitta-rs restart**: after dropping and recreating the database, chitta-rs must be restarted so its connection pool reconnects and sqlx migrations run on the fresh DB.

## What to download before terminating the pod

```bash
cd /workspace/agent-memory-benchmark

tar czf /workspace/amb-results.tar.gz \
    outputs/beam/chitta-mcp/ \
    outputs/personamem/chitta-mcp/

# From your local machine:
runpodctl receive /workspace/amb-results.tar.gz
# or
scp -P <port> root@<pod-ip>:/workspace/amb-results.tar.gz .
```

## Estimated costs

| Dataset | Queries | Gemini cost | GPU time | Total pod time |
|---------|---------|-------------|----------|---------------|
| PersonaMem 32k | 589 | ~$0.05 | ~2 min embed | ~15 min |
| BEAM 100k | 2000 | ~$0.20 | ~5 min embed | ~45 min |
| BEAM 500k | 2000 | ~$0.20 | ~10 min embed | ~60 min |
| BEAM 1M | 2000 | ~$0.20 | ~15 min embed | ~90 min |
| LifeBench en | 2003 | $0 (retrieval-only) | ~20 min embed | ~30 min |
| LifeBench sweep (7 configs) | 2003 × 7 | $0 | ~140 min embed | ~3.5 hr |

GPU time estimates assume L40S. CPU ONNX is 10-50x slower.
