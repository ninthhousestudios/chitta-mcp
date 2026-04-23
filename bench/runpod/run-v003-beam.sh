#!/usr/bin/env bash
set -euo pipefail

# v0.0.3 BEAM runs: control (w=0) + best PersonaMem w (0.05).
#
# PersonaMem sweep found w=0.05 as best (64.18% vs 63.67% control).
# This script runs BEAM at both weights to see if the lift transfers.
#
# Usage:
#   bash run-v003-beam.sh              # full runs
#   bash run-v003-beam.sh 50           # quick test (50 queries each)
#
# Recovery: skips any config whose result file already exists.

QUERY_LIMIT="${1:-0}"

WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
RESULTS_DIR="$WORK_DIR/v003-beam-results"

RECENCY_HALF_LIFE="${RECENCY_HALF_LIFE:-30.0}"
BEAM_K="${BEAM_K:-20}"
BEAM_SPLIT="100k"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4
export OMB_ANSWER_LLM=gemini
export OMB_JUDGE_LLM=gemini
export CHITTA_K="$BEAM_K"

mkdir -p "$RESULTS_DIR"

# ── Preflight checks ───────────────────────────────────────────────

if [ ! -f "$CHITTA_DIR/target/release/chitta-rs" ]; then
    echo "ERROR: chitta-rs binary not found — run setup.sh first"
    exit 1
fi

if [ ! -d "$AMB_DIR" ]; then
    echo "ERROR: agent-memory-benchmark not found at $AMB_DIR — run setup.sh first"
    exit 1
fi

pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

# ── Load env files once ────────────────────────────────────────────

cd "$AMB_DIR"
if [ -f .env ]; then
    set -a; . ./.env; set +a
fi

cd "$CHITTA_DIR"
if [ -f .env ]; then
    set -a; . ./.env; set +a
fi
if [ -z "${ORT_DYLIB_PATH:-}" ] || [ ! -f "${ORT_DYLIB_PATH:-}" ]; then
    echo "ERROR: ORT_DYLIB_PATH not set or file not found — run setup.sh first"
    exit 1
fi
export LD_LIBRARY_PATH="$(dirname "$ORT_DYLIB_PATH")${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$AMB_DIR"

# ── Helpers ─────────────────────────────────────────────────────────

_reset_and_start() {
    pkill -f "chitta-rs --http" || true
    sleep 2

    su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
    su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
    su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

    cd "$CHITTA_DIR"
    RUST_LOG="${RUST_LOG:-chitta_rs=info,ort=debug}" \
        CHITTA_RECENCY_WEIGHT="$CURRENT_W" \
        CHITTA_RECENCY_HALF_LIFE_DAYS="$RECENCY_HALF_LIFE" \
        ./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
    sleep 5
    cd "$AMB_DIR"
}

_find_result() {
    local name="$1" split="$2"
    local base="$AMB_DIR/outputs/beam/$name/rag/${split}"
    if [ -f "${base}.json.gz" ]; then
        echo "${base}.json.gz"
    elif [ -f "${base}.json" ]; then
        echo "${base}.json"
    fi
}

_run_config() {
    local name="$1" w="$2"

    local existing
    existing=$(_find_result "$name" "$BEAM_SPLIT")
    if [ -n "$existing" ]; then
        echo "[$name] Already exists at $existing — skipping"
        cp "$existing" "$RESULTS_DIR/" 2>/dev/null || true
        return 0
    fi

    # Also skip if we already copied it
    if ls "$RESULTS_DIR/${name}".* &>/dev/null; then
        echo "[$name] Already in results dir — skipping"
        return 0
    fi

    echo ""
    echo "============================================================"
    echo "[$name] beam $BEAM_SPLIT  w=$w  k=$BEAM_K  half_life=$RECENCY_HALF_LIFE"
    echo "============================================================"

    CURRENT_W="$w"
    _reset_and_start

    local start_time
    start_time=$(date +%s)

    local cmd=(uv run omb run
        --dataset beam
        --split "$BEAM_SPLIT"
        --memory chitta-mcp
        --llm gemini
        --name "$name")

    if [ "$QUERY_LIMIT" -gt 0 ]; then
        cmd+=(--query-limit "$QUERY_LIMIT")
    fi

    "${cmd[@]}"

    local end_time elapsed
    end_time=$(date +%s)
    elapsed=$(( end_time - start_time ))
    echo "[$name] Done in ${elapsed}s"

    local result
    result=$(_find_result "$name" "$BEAM_SPLIT")
    if [ -n "$result" ]; then
        cp "$result" "$RESULTS_DIR/"
        echo "  Saved to $RESULTS_DIR/"
    else
        echo "  WARN: result not found"
        echo "  Checked: $AMB_DIR/outputs/beam/$name/rag/${BEAM_SPLIT}.json{,.gz}"
    fi
}

# ── Header ──────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  v0.0.3 BEAM: control (w=0.0) + best PersonaMem w (w=0.05)"
echo "  k: $BEAM_K"
echo "  half-life: $RECENCY_HALF_LIFE days"
if [ "$QUERY_LIMIT" -gt 0 ]; then
    echo "  Query limit: $QUERY_LIMIT per config"
else
    echo "  Query limit: none (full runs)"
fi
echo "================================================================"

TOTAL_START=$(date +%s)

# ── Run 1: control w=0.0 ────────────────────────────────────────────

CURRENT_W="0.0"
_run_config "v003-beam-rw00" "0.0"

# ── Run 2: best w=0.05 ──────────────────────────────────────────────

CURRENT_W="0.05"
_run_config "v003-beam-rw005" "0.05"

# ── Summary ─────────────────────────────────────────────────────────

pkill -f "chitta-rs --http" || true

TOTAL_END=$(date +%s)
TOTAL_ELAPSED=$(( TOTAL_END - TOTAL_START ))

echo ""
echo "================================================================"
echo "  BEAM runs complete — total wall time: ${TOTAL_ELAPSED}s"
echo "================================================================"

export RESULTS_DIR
python3 <<'PY'
import gzip, json, os, glob, sys

results_dir = os.environ.get("RESULTS_DIR", "")
if not results_dir or not os.path.isdir(results_dir):
    print("No results directory found")
    sys.exit(0)

rows = []
for path in sorted(glob.glob(os.path.join(results_dir, "v003-beam-*"))):
    name = os.path.basename(path).split(".")[0]
    try:
        opener = gzip.open if path.endswith(".gz") else open
        with opener(path, "rt") as f:
            d = json.load(f)
        rows.append((
            name,
            d.get("accuracy", 0),
            d.get("total_queries", 0),
            d.get("correct", 0),
            d.get("avg_context_tokens", 0),
            d.get("avg_retrieve_time_ms", 0),
            d.get("ingestion_time_ms", 0),
        ))
    except Exception as e:
        rows.append((name, None, 0, 0, 0, 0, 0))

if not rows:
    print("No results found")
    sys.exit(0)

print()
print(f"{'config':<25} {'accuracy':>8} {'correct':>9} {'avg_ctx':>8} {'retr_ms':>8} {'ingest':>8}")
print("-" * 70)

for name, acc, total, correct, ctx, retr, ing in rows:
    if acc is None:
        print(f"{name:<25} {'ERROR':>8}")
        continue
    print(
        f"{name:<25} "
        f"{acc*100:>7.1f}% "
        f"{correct:>4}/{total:<4} "
        f"{ctx:>8.0f} "
        f"{retr:>8.1f} "
        f"{ing/1000:>7.1f}s"
    )

print()
print("  PersonaMem results:  w=0.05 64.18%  w=0.0 63.67%  (best: w=0.05, +0.51pp)")
print("  v0.0.2 baselines:    PersonaMem 64.3%  BEAM 66.6%")
PY

echo ""
echo "Results saved to: $RESULTS_DIR/"
