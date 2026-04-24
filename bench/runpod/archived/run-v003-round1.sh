#!/usr/bin/env bash
set -euo pipefail

# v0.0.3 Round 1: adapter-only parameter sweeps.
#
# Sweeps k values on both benchmarks and chunk sizes on PersonaMem.
# No chitta-rs code changes required — all variation is via env vars.
#
# Usage:
#   bash run-v003-round1.sh              # full runs (no query limit)
#   bash run-v003-round1.sh 50           # quick exploration (50 queries each)
#
# Override sweep grid via env:
#   K_VALUES="10 20 30 40"
#   PM_CHUNK_SIZES="256 512 1024"
#
# Recovery: skips any config whose result file already exists.
#           Re-run the same command to resume after a crash.

QUERY_LIMIT="${1:-0}"

WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
RESULTS_DIR="$WORK_DIR/v003-round1-results"

K_VALUES="${K_VALUES:-10 20 30 40}"
PM_CHUNK_SIZES="${PM_CHUNK_SIZES:-256 512 1024}"
PM_SPLIT="32k"
BEAM_SPLIT="100k"
OVERLAP_RATIO=0.125
TURNS_PER_CHUNK="${TURNS_PER_CHUNK:-4}"
OVERLAP_TURNS="${OVERLAP_TURNS:-1}"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4
export OMB_ANSWER_LLM=gemini
export OMB_JUDGE_LLM=gemini

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
        ./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
    sleep 5
    cd "$AMB_DIR"
}

_result_path() {
    local name="$1" dataset="$2" split="$3"
    echo "$AMB_DIR/outputs/$dataset/$name/rag/${split}.json.gz"
}

_run_config() {
    local name="$1" dataset="$2" split="$3"

    local result_path
    result_path=$(_result_path "$name" "$dataset" "$split")

    if [ -f "$result_path" ]; then
        echo "[$name] Already exists — skipping"
        cp "$result_path" "$RESULTS_DIR/${name}.json.gz" 2>/dev/null || true
        return 0
    fi

    echo ""
    echo "============================================================"
    echo "[$name] $dataset $split"
    echo "  CHITTA_K=${CHITTA_K:-default}"
    [ -n "${CHITTA_CHUNK_SIZE:-}" ] && echo "  CHITTA_CHUNK_SIZE=$CHITTA_CHUNK_SIZE  CHITTA_CHUNK_OVERLAP=${CHITTA_CHUNK_OVERLAP:-}"
    echo "============================================================"

    _reset_and_start

    local start_time
    start_time=$(date +%s)

    local cmd=(uv run omb run
        --dataset "$dataset"
        --split "$split"
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

    if [ -f "$result_path" ]; then
        cp "$result_path" "$RESULTS_DIR/${name}.json.gz"
        echo "  Saved: $RESULTS_DIR/${name}.json.gz"
    else
        echo "  WARN: result not found at $result_path"
    fi
}

# ── Header ──────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  v0.0.3 Round 1: adapter-only sweeps"
echo "  k values: $K_VALUES"
echo "  PersonaMem chunk sizes: $PM_CHUNK_SIZES"
if [ "$QUERY_LIMIT" -gt 0 ]; then
    echo "  Query limit: $QUERY_LIMIT per config"
else
    echo "  Query limit: none (full runs)"
fi
echo "================================================================"

TOTAL_START=$(date +%s)

# ── Phase 1: PersonaMem k sweep (default chunking) ─────────────────

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║  Phase 1/3: PersonaMem $PM_SPLIT — k sweep                 ║"
echo "╚════════════════════════════════════════════════════════════╝"

unset CHITTA_CHUNK_SIZE CHITTA_CHUNK_OVERLAP 2>/dev/null || true
for K in $K_VALUES; do
    export CHITTA_K="$K"
    _run_config "v003-pm-k${K}" "personamem" "$PM_SPLIT"
done

# ── Phase 2: PersonaMem chunk-size sweep (k=10) ────────────────────

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║  Phase 2/3: PersonaMem $PM_SPLIT — chunk-size sweep         ║"
echo "╚════════════════════════════════════════════════════════════╝"

export CHITTA_K="10"
export CHITTA_TURNS_PER_CHUNK="$TURNS_PER_CHUNK"
export CHITTA_OVERLAP_TURNS="$OVERLAP_TURNS"
for CS in $PM_CHUNK_SIZES; do
    OV=$(python3 -c "print(int($CS * $OVERLAP_RATIO))")
    export CHITTA_CHUNK_SIZE="$CS"
    export CHITTA_CHUNK_OVERLAP="$OV"
    _run_config "v003-pm-k10-cs${CS}" "personamem" "$PM_SPLIT"
done
unset CHITTA_CHUNK_SIZE CHITTA_CHUNK_OVERLAP 2>/dev/null || true

# ── Phase 3: BEAM k sweep ──────────────────────────────────────────

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║  Phase 3/3: BEAM $BEAM_SPLIT — k sweep                     ║"
echo "╚════════════════════════════════════════════════════════════╝"

for K in $K_VALUES; do
    export CHITTA_K="$K"
    _run_config "v003-beam-k${K}" "beam" "$BEAM_SPLIT"
done

# ── Summary ─────────────────────────────────────────────────────────

TOTAL_END=$(date +%s)
TOTAL_ELAPSED=$(( TOTAL_END - TOTAL_START ))

echo ""
echo "================================================================"
echo "  Round 1 complete — total wall time: ${TOTAL_ELAPSED}s"
echo "================================================================"

export RESULTS_DIR
python3 <<'PY'
import gzip, json, os, glob, sys

results_dir = os.environ.get("RESULTS_DIR", "")
if not results_dir or not os.path.isdir(results_dir):
    print("No results directory found")
    sys.exit(0)

rows = []
for path in sorted(glob.glob(os.path.join(results_dir, "*.json.gz"))):
    name = os.path.basename(path).replace(".json.gz", "")
    try:
        with gzip.open(path, "rt") as f:
            d = json.load(f)
        rows.append((
            name,
            d.get("dataset", "?"),
            d.get("accuracy", 0),
            d.get("total_queries", 0),
            d.get("correct", 0),
            d.get("avg_context_tokens", 0),
            d.get("avg_retrieve_time_ms", 0),
            d.get("ingestion_time_ms", 0),
        ))
    except Exception as e:
        rows.append((name, "?", None, 0, 0, 0, 0, 0))

if not rows:
    print("No results found")
    sys.exit(0)

print()
print(f"{'config':<25} {'dataset':<12} {'accuracy':>8} {'correct':>9} {'avg_ctx':>8} {'retr_ms':>8} {'ingest':>8}")
print("-" * 82)

for name, ds, acc, total, correct, ctx, retr, ing in rows:
    if acc is None:
        print(f"{name:<25} {'ERROR':>8}")
        continue
    print(
        f"{name:<25} "
        f"{ds:<12} "
        f"{acc*100:>7.1f}% "
        f"{correct:>4}/{total:<4} "
        f"{ctx:>8.0f} "
        f"{retr:>8.1f} "
        f"{ing/1000:>7.1f}s"
    )

# Best per dataset
print()
for ds in ["personamem", "beam"]:
    ds_rows = [(n, a) for n, d, a, *_ in rows if d == ds and a is not None]
    if ds_rows:
        best = max(ds_rows, key=lambda x: x[1])
        print(f"  Best {ds}: {best[0]} ({best[1]*100:.1f}%)")

# v0.0.2 baselines for reference
print()
print("  v0.0.2 baselines: PersonaMem 64.3%, BEAM 66.6%")
PY

echo ""
echo "Results saved to: $RESULTS_DIR/"
echo ""
echo "Next steps:"
echo "  1. Review the summary above"
echo "  2. If best chunk != default and best k != 10, run the combined config"
echo "  3. Proceed to Round 2 (recency-weighted scoring in chitta-rs)"
