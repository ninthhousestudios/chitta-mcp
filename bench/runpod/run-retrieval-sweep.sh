#!/usr/bin/env bash
set -euo pipefail

# Retrieval-only parameter sweep — zero LLM cost.
#
# Usage:
#   bash run-retrieval-sweep.sh [DATASET] [SPLIT]
#
# Defaults: personamem 32k
#
# Edit the CONFIGS array below to define sweep parameters.
# Each entry is "NAME:VAR=VAL VAR=VAL ..."
#
# All index types (dense, FTS, sparse) are built at ingestion time
# regardless of config flags. The sweep ingests once with the first
# config, then restarts chitta-rs with different retrieval flags for
# each subsequent config (--skip-ingestion).

DATASET="${1:-personamem}"
SPLIT="${2:-32k}"
WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
EVAL_SCRIPT="$CHITTA_DIR/bench/retrieval-eval.py"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4

pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

# ── Sweep configs ────────────────────────────────────────────────────
# Format: "RUN_NAME:ENV_VAR=VALUE ENV_VAR=VALUE ..."
CONFIGS=(
    "dense-only:CHITTA_K=20"
    "dense-fts:CHITTA_K=20 CHITTA_RRF_FTS=true"
    "dense-sparse:CHITTA_K=20 CHITTA_RRF_SPARSE=true"
    "dense-fts-sparse:CHITTA_K=20 CHITTA_RRF_FTS=true CHITTA_RRF_SPARSE=true"
    "rrf-k20:CHITTA_K=20 CHITTA_RRF_FTS=true CHITTA_RRF_SPARSE=true CHITTA_RRF_K=20"
    "rrf-k60:CHITTA_K=20 CHITTA_RRF_FTS=true CHITTA_RRF_SPARSE=true CHITTA_RRF_K=60"
    "rrf-k120:CHITTA_K=20 CHITTA_RRF_FTS=true CHITTA_RRF_SPARSE=true CHITTA_RRF_K=120"
)

# ── Helpers ──────────────────────────────────────────────────────────

_reset_and_start() {
    pkill -f "chitta-rs --http" || true
    sleep 2

    su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
    su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
    su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

    _start_server
}

_restart_server() {
    pkill -f "chitta-rs --http" || true
    sleep 2
    _start_server
}

_start_server() {
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
    cd "$AMB_DIR"
}

# ── Main ─────────────────────────────────────────────────────────────

cd "$AMB_DIR"
if [ -f .env ]; then set -a; . ./.env; set +a; fi

echo "=== Retrieval sweep: $DATASET/$SPLIT ==="
echo "Configs: ${#CONFIGS[@]}"
echo ""

SWEEP_START=$(date +%s)

# Check if DB already has data (e.g. from a prior interrupted run).
DB_HAS_DATA=false
ROW_COUNT=$(su - postgres -c "psql -d chitta_beam -tAc 'SELECT count(*) FROM memories;'" 2>/dev/null || echo "0")
if [ "$ROW_COUNT" -gt 0 ] 2>/dev/null; then
    DB_HAS_DATA=true
    echo "DB already has $ROW_COUNT memories — skipping ingestion for all configs."
    echo ""
fi

FIRST=true

for config_entry in "${CONFIGS[@]}"; do
    RUN_NAME="${config_entry%%:*}"
    ENV_PAIRS="${config_entry#*:}"

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Config: $RUN_NAME ($ENV_PAIRS)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Reset RRF-related vars to prevent leakage between iterations.
    unset CHITTA_K CHITTA_RRF_FTS CHITTA_RRF_SPARSE CHITTA_RRF_K CHITTA_RRF_CANDIDATES

    # Apply env overrides
    for pair in $ENV_PAIRS; do
        export "$pair"
    done

    SKIP_INGEST_FLAG=()
    if [ "$DB_HAS_DATA" = true ]; then
        _restart_server
        SKIP_INGEST_FLAG=(--skip-ingestion)
    elif [ "$FIRST" = true ]; then
        _reset_and_start
        FIRST=false
    else
        _restart_server
        SKIP_INGEST_FLAG=(--skip-ingestion)
    fi

    RUN_START=$(date +%s)
    uv run python "$EVAL_SCRIPT" \
        --dataset "$DATASET" \
        --split "$SPLIT" \
        --name "$RUN_NAME" \
        --amb-dir "$AMB_DIR" \
        "${SKIP_INGEST_FLAG[@]}"
    RUN_END=$(date +%s)

    echo "  [$RUN_NAME] completed in $((RUN_END - RUN_START))s"
    echo ""
done

SWEEP_END=$(date +%s)
echo "=== Sweep complete: $((SWEEP_END - SWEEP_START))s total ==="
echo "Results in: $AMB_DIR/outputs/$DATASET/*/retrieval/$SPLIT.json"
