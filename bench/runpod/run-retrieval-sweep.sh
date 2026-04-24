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
# The DB is reset and chitta-rs restarted for each config.
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

for config_entry in "${CONFIGS[@]}"; do
    RUN_NAME="${config_entry%%:*}"
    ENV_PAIRS="${config_entry#*:}"

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Config: $RUN_NAME ($ENV_PAIRS)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Apply env overrides
    for pair in $ENV_PAIRS; do
        export "$pair"
    done

    _reset_and_start

    RUN_START=$(date +%s)
    uv run python "$EVAL_SCRIPT" \
        --dataset "$DATASET" \
        --split "$SPLIT" \
        --name "$RUN_NAME" \
        --amb-dir "$AMB_DIR"
    RUN_END=$(date +%s)

    echo "  [$RUN_NAME] completed in $((RUN_END - RUN_START))s"
    echo ""
done

SWEEP_END=$(date +%s)
echo "=== Sweep complete: $((SWEEP_END - SWEEP_START))s total ==="
echo "Results in: $AMB_DIR/outputs/$DATASET/*/retrieval/$SPLIT.json"
