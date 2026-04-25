#!/usr/bin/env bash
set -euo pipefail

# Deduplication sweep — dense-only with doc_id dedup + k variations.
#
# Runs both BEAM (100k) and LifeBench (en) sequentially. For each dataset:
# ingests once with the first config, then restarts chitta-rs with different
# retrieval flags for each subsequent config (--skip-ingestion).
#
# Usage:
#   bash run-dedup-sweep.sh

WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
EVAL_SCRIPT="$CHITTA_DIR/bench/retrieval-eval.py"
DB_NAME="chitta_bench"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4

pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

# ── Datasets ────────────────────────────────────────────────────────
DATASETS=(
    "beam:100k"
    "lifebench:en"
)

# ── Sweep configs ───────────────────────────────────────────────────
# Format: "RUN_NAME:ENV_VAR=VALUE ENV_VAR=VALUE ..."
# All share same chunking (512/64) so one ingestion per dataset suffices.
CONFIGS=(
    "dense-only:CHITTA_K=20"
    "dense-dedup:CHITTA_K=20 CHITTA_DEDUP_FIELD=doc_id"
    "dense-dedup-k30:CHITTA_K=30 CHITTA_DEDUP_FIELD=doc_id"
    "dense-dedup-k40:CHITTA_K=40 CHITTA_DEDUP_FIELD=doc_id"
)

# ── Helpers ─────────────────────────────────────────────────────────

_reset_db() {
    pkill -f "chitta-rs --http" || true
    sleep 2

    su - postgres -c "psql -c 'DROP DATABASE IF EXISTS $DB_NAME;'"
    su - postgres -c "psql -c 'CREATE DATABASE $DB_NAME OWNER chitta;'"
    su - postgres -c "psql -d $DB_NAME -c 'CREATE EXTENSION IF NOT EXISTS vector;'"
}

_restart_server() {
    pkill -f "chitta-rs --http" || true
    sleep 2
    _start_server
}

_start_server() {
    cd "$CHITTA_DIR"
    if [ -f .env ]; then set -a; . ./.env; set +a; fi
    export DATABASE_URL="postgres://chitta:chitta@localhost/$DB_NAME"
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

_clear_retrieval_env() {
    unset CHITTA_K CHITTA_DEDUP_FIELD CHITTA_DEDUP_FETCH_FACTOR \
          CHITTA_RRF_FTS CHITTA_RRF_SPARSE CHITTA_RRF_K CHITTA_RRF_CANDIDATES \
          2>/dev/null || true
}

# ── Main ────────────────────────────────────────────────────────────

cd "$AMB_DIR"
if [ -f .env ]; then set -a; . ./.env; set +a; fi

SWEEP_START=$(date +%s)
echo "=== Dedup sweep: ${#DATASETS[@]} datasets × ${#CONFIGS[@]} configs ==="
echo ""

for dataset_entry in "${DATASETS[@]}"; do
    DATASET="${dataset_entry%%:*}"
    SPLIT="${dataset_entry#*:}"

    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║  Dataset: $DATASET/$SPLIT"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""

    _reset_db
    FIRST=true

    for config_entry in "${CONFIGS[@]}"; do
        RUN_NAME="${config_entry%%:*}"
        ENV_PAIRS="${config_entry#*:}"

        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "  Config: $RUN_NAME ($ENV_PAIRS)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

        _clear_retrieval_env

        for pair in $ENV_PAIRS; do
            export "$pair"
        done

        SKIP_INGEST_FLAG=()
        if [ "$FIRST" = true ]; then
            _start_server
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
done

SWEEP_END=$(date +%s)
echo "=== Sweep complete: $((SWEEP_END - SWEEP_START))s total ==="
echo "Results in: $AMB_DIR/outputs/*/*/retrieval/*.json"
