#!/usr/bin/env bash
set -euo pipefail

# v0.0.3 Round 2: recency-weighted scoring sweep.
#
# Sweeps CHITTA_RECENCY_WEIGHT on PersonaMem (full grid), then runs
# BEAM at w=0 (control) and the best w from PersonaMem.
#
# Usage:
#   bash run-v003-round2.sh              # full runs (no query limit)
#   bash run-v003-round2.sh 50           # quick exploration (50 queries each)
#
# Override sweep grid via env:
#   W_VALUES="0.0 0.1 0.2 0.3"
#   RECENCY_HALF_LIFE="30.0"
#
# Recovery: skips any config whose result file already exists.

QUERY_LIMIT="${1:-0}"

WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
RESULTS_DIR="$WORK_DIR/v003-round2-results"

W_VALUES="${W_VALUES:-0.0 0.05 0.1 0.2 0.3}"
RECENCY_HALF_LIFE="${RECENCY_HALF_LIFE:-30.0}"
SWEEP_K="${SWEEP_K:-20}"

PM_SPLIT="32k"
BEAM_SPLIT="100k"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4
export OMB_ANSWER_LLM=gemini
export OMB_JUDGE_LLM=gemini
export CHITTA_K="$SWEEP_K"

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
    echo "  CHITTA_K=$SWEEP_K"
    echo "  CHITTA_RECENCY_WEIGHT=$CURRENT_W"
    echo "  CHITTA_RECENCY_HALF_LIFE_DAYS=$RECENCY_HALF_LIFE"
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
echo "  v0.0.3 Round 2: recency-weighted scoring sweep"
echo "  w values: $W_VALUES"
echo "  half-life: $RECENCY_HALF_LIFE days"
echo "  k: $SWEEP_K"
if [ "$QUERY_LIMIT" -gt 0 ]; then
    echo "  Query limit: $QUERY_LIMIT per config"
else
    echo "  Query limit: none (full runs)"
fi
echo "================================================================"

TOTAL_START=$(date +%s)

# ── Phase 1: PersonaMem full w sweep ──────────────────────────────

echo ""
echo "╔════════════════════════════════════════════════���═══════════╗"
echo "║  Phase 1/2: PersonaMem $PM_SPLIT — recency weight sweep     ║"
echo "╚════════════════════════════════════════════════════════════╝"

BEST_W="0.0"
BEST_ACC=0

for W in $W_VALUES; do
    CURRENT_W="$W"
    # Sanitize w for filename: 0.05 -> 005
    W_LABEL=$(echo "$W" | tr -d '.')
    _run_config "v003-pm-rw${W_LABEL}" "personamem" "$PM_SPLIT"
done

# ── Find best w from PersonaMem results ────────────────────────────

echo ""
echo "Finding best w from PersonaMem results..."
BEST_W=$(python3 <<'PY'
import gzip, json, os, glob

results_dir = os.environ["RESULTS_DIR"]
best_w = "0.0"
best_acc = 0
for path in glob.glob(os.path.join(results_dir, "v003-pm-rw*.json.gz")):
    name = os.path.basename(path).replace(".json.gz", "")
    try:
        with gzip.open(path, "rt") as f:
            d = json.load(f)
        acc = d.get("accuracy", 0)
        if acc > best_acc:
            best_acc = acc
            # Extract w label from name, e.g. v003-pm-rw01 -> 01
            w_label = name.replace("v003-pm-rw", "")
            best_w = w_label
    except Exception:
        pass
print(best_w)
PY
)

echo "  Best w label: $BEST_W"

# ── Phase 2: BEAM — control (w=0) + best w ────────────────────────

echo ""
echo "╔════════════════════════════════════════════════════════════╗"
echo "║  Phase 2/2: BEAM $BEAM_SPLIT — control + best w             ║"
echo "╚════════════════════════════════════════════════════════════╝"

# Control: w=0
CURRENT_W="0.0"
_run_config "v003-beam-rw00" "beam" "$BEAM_SPLIT"

# Best w (skip if best was 0.0)
if [ "$BEST_W" != "00" ] && [ "$BEST_W" != "0" ]; then
    # Reconstruct the float from the label
    CURRENT_W=$(python3 -c "
w_map = {'005': '0.05', '01': '0.1', '02': '0.2', '03': '0.3', '00': '0.0'}
print(w_map.get('$BEST_W', '0.0'))
")
    _run_config "v003-beam-rw${BEST_W}" "beam" "$BEAM_SPLIT"
fi

# ── Summary ─────────────────────────────────────────────────────────

TOTAL_END=$(date +%s)
TOTAL_ELAPSED=$(( TOTAL_END - TOTAL_START ))

echo ""
echo "================================================================"
echo "  Round 2 complete — total wall time: ${TOTAL_ELAPSED}s"
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

# Baselines
print()
print("  v0.0.2 baselines: PersonaMem 64.3%, BEAM 66.6%")
print("  Round 1 best: PersonaMem k=40 64.3% (pure cosine, no recency)")
PY

echo ""
echo "Results saved to: $RESULTS_DIR/"
echo ""
echo "Next steps:"
echo "  1. Review the summary above"
echo "  2. If recency w > 0 beats control, it goes into v0.0.3"
echo "  3. Proceed to Round 3 based on decision gate in roadmap"
