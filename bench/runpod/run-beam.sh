#!/usr/bin/env bash
set -euo pipefail

# Run BEAM benchmarks on RunPod with chitta-rs.
# Usage:
#   bash run-beam.sh [SPLIT]
#
# SPLIT defaults to 100k. Options: 100k, 500k, 1m, 10m

SPLIT="${1:-100k}"
WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4

# Ensure postgres is running
pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

cd "$AMB_DIR"

if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi

_reset_and_start() {
    pkill -f "chitta-rs --http" || true
    sleep 2

    echo "--- Resetting DB ---"
    su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
    su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
    su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

    echo "Starting chitta-rs..."
    cd "$CHITTA_DIR"
    if [ -f .env ]; then
        set -a; . ./.env; set +a
    fi
    if [ -n "$ORT_DYLIB_PATH" ] && [ ! -f "$ORT_DYLIB_PATH" ]; then
        echo "ERROR: ORT_DYLIB_PATH=$ORT_DYLIB_PATH does not exist — re-run setup.sh"
        exit 1
    fi
    export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$(dirname "$ORT_DYLIB_PATH")"
    echo "LD_LIBRARY_PATH=$LD_LIBRARY_PATH"
    RUST_LOG="${RUST_LOG:-chitta_rs=info,ort=debug}" \
        ./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
    sleep 5
    cd "$AMB_DIR"
}

echo "=== BEAM $SPLIT benchmark ==="
echo "Provider: chitta-mcp (chitta-rs HTTP)"
echo ""

# ── Smoke test (2 conversations) ────────────────────────────────────
_reset_and_start

echo "--- Smoke test (--query-limit 2) ---"
uv run omb run \
    --dataset beam \
    --split "$SPLIT" \
    --memory chitta-mcp \
    --llm gemini \
    --name chitta-mcp \
    --query-limit 2

echo ""
echo "--- Smoke test passed ---"
su - postgres -c "psql -d chitta_beam -c \"SELECT pg_size_pretty(pg_database_size('chitta_beam'));\""
echo ""

read -p "Run full benchmark? [y/N] " -n 1 -r
echo ""
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 0
fi

# ── Clean DB for full run ────────────────────────────────────────────
_reset_and_start

# ── Full run ─────────────────────────────────────────────────────────
echo "--- Full BEAM $SPLIT run ---"
START_TIME=$(date +%s)

uv run omb run \
    --dataset beam \
    --split "$SPLIT" \
    --memory chitta-mcp \
    --llm gemini \
    --name chitta-mcp

END_TIME=$(date +%s)
ELAPSED=$(( END_TIME - START_TIME ))
echo ""
echo "--- Completed in ${ELAPSED}s ---"

# ── VACUUM and report DB size ────────────────────────────────────────
echo "Running VACUUM FULL..."
su - postgres -c "psql -d chitta_beam -c 'VACUUM FULL;'"

echo ""
echo "--- Final DB size ---"
su - postgres -c "psql -d chitta_beam -c \"SELECT pg_size_pretty(pg_database_size('chitta_beam'));\""

echo ""
echo "--- Table sizes ---"
su - postgres -c "psql -d chitta_beam -c \"
SELECT relname AS table,
       pg_size_pretty(pg_total_relation_size(c.oid)) AS total
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind = 'r'
ORDER BY pg_total_relation_size(c.oid) DESC;
\""

echo ""
echo "--- Results saved to $AMB_DIR/outputs/beam/chitta-mcp/ ---"
echo "--- Wall time: ${ELAPSED}s ---"
