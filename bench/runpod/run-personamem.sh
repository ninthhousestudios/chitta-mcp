#!/usr/bin/env bash
set -euo pipefail

# Run PersonaMem benchmarks on RunPod with chitta-rs.
# Usage:
#   bash run-personamem.sh [SPLIT]
#
# SPLIT defaults to 32k. Options: 32k, 128k, 1M

SPLIT="${1:-32k}"
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

echo "=== PersonaMem $SPLIT benchmark ==="
echo "Provider: chitta-mcp (chitta-rs HTTP)"
echo ""

# ── Stop chitta-rs so we can reset the DB ────────────────────────────
pkill -f "chitta-rs --http" || true
sleep 2

# ── Clean DB ─────────────────────────────────────────────────────────
echo "--- Resetting DB ---"
su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

# ── Start chitta-rs (applies migrations on connect) ──────────────────
echo "Starting chitta-rs..."
cd "$CHITTA_DIR"
if [ -f .env ]; then
    ORT_PATH=$(grep '^ORT_DYLIB_PATH=' .env | cut -d= -f2)
    if [ -n "$ORT_PATH" ] && [ ! -f "$ORT_PATH" ]; then
        echo "WARNING: ORT_DYLIB_PATH=$ORT_PATH does not exist, re-resolving..."
        NEW_ORT=$(find /usr/local/lib/onnxruntime /usr/local/lib /usr/lib -name "libonnxruntime.so*" ! -name "*providers*" 2>/dev/null | head -1 || true)
        if [ -z "$NEW_ORT" ]; then
            echo "ERROR: Cannot find libonnxruntime.so — re-run setup.sh"
            exit 1
        fi
        sed -i "s|^ORT_DYLIB_PATH=.*|ORT_DYLIB_PATH=$NEW_ORT|" .env
        echo "Updated ORT_DYLIB_PATH=$NEW_ORT"
    fi
fi
./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
sleep 5
cd "$AMB_DIR"

# ── Run ──────────────────────────────────────────────────────────────
echo "--- PersonaMem $SPLIT ---"
START_TIME=$(date +%s)

uv run omb run \
    --dataset personamem \
    --split "$SPLIT" \
    --memory chitta-mcp \
    --llm gemini \
    --name chitta-mcp

END_TIME=$(date +%s)
ELAPSED=$(( END_TIME - START_TIME ))

# ── VACUUM and report ────────────────────────────────────────────────
echo ""
echo "Running VACUUM FULL..."
su - postgres -c "psql -d chitta_beam -c 'VACUUM FULL;'"

echo ""
echo "--- Final DB size ---"
su - postgres -c "psql -d chitta_beam -c \"SELECT pg_size_pretty(pg_database_size('chitta_beam'));\""

echo ""
echo "--- Results saved to $AMB_DIR/outputs/personamem/chitta-mcp/ ---"
echo "--- Wall time: ${ELAPSED}s ---"
