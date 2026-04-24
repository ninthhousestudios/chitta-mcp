#!/usr/bin/env bash
set -euo pipefail

# Retrieval-only PersonaMem eval — zero LLM cost.
# Usage:
#   bash run-retrieval-personamem.sh [SPLIT] [QUERY_LIMIT]
#
# SPLIT defaults to 32k. Options: 32k, 128k, 1M

SPLIT="${1:-32k}"
QUERY_LIMIT="${2:-}"
WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
EVAL_SCRIPT="$CHITTA_DIR/bench/retrieval-eval.py"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4

pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

# ── Stop chitta-rs and reset DB ──────────────────────────────────────
pkill -f "chitta-rs --http" || true
sleep 2

echo "--- Resetting DB ---"
su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

# ── Start chitta-rs ──────────────────────────────────────────────────
echo "Starting chitta-rs..."
cd "$CHITTA_DIR"
if [ -f .env ]; then set -a; . ./.env; set +a; fi
if [ -n "${ORT_DYLIB_PATH:-}" ] && [ ! -f "$ORT_DYLIB_PATH" ]; then
    echo "ERROR: ORT_DYLIB_PATH=$ORT_DYLIB_PATH does not exist"
    exit 1
fi
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$(dirname "$ORT_DYLIB_PATH")"
RUST_LOG="${RUST_LOG:-chitta_rs=info}" \
    ./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
sleep 5

# ── Run retrieval eval ───────────────────────────────────────────────
cd "$AMB_DIR"
if [ -f .env ]; then set -a; . ./.env; set +a; fi

echo "=== PersonaMem $SPLIT retrieval-only eval ==="

EXTRA_ARGS=()
if [ -n "$QUERY_LIMIT" ]; then
    EXTRA_ARGS+=(--query-limit "$QUERY_LIMIT")
fi

START_TIME=$(date +%s)
uv run python "$EVAL_SCRIPT" \
    --dataset personamem \
    --split "$SPLIT" \
    --amb-dir "$AMB_DIR" \
    "${EXTRA_ARGS[@]}"
END_TIME=$(date +%s)

echo ""
echo "--- Wall time: $((END_TIME - START_TIME))s ---"
