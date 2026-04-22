#!/usr/bin/env bash
set -euo pipefail

# Chunk-size sweep on PersonaMem with chitta-rs.
#
# For each chunk_size in CHUNK_SIZES, drop+recreate the DB, restart
# chitta-rs, ingest with that chunk size, run queries, and save results.
#
# Usage:
#   bash run-personamem-sweep.sh [SPLIT] [QUERY_LIMIT]
# Defaults:
#   SPLIT=32k  QUERY_LIMIT=50
#
# Override via env:
#   CHUNK_SIZES="256 512 1024"   chunk-size grid (in tokens)
#   OVERLAP_RATIO=0.125          overlap = chunk_size * ratio
#   TURNS_PER_CHUNK=4            message-window size
#   OVERLAP_TURNS=1              message-window overlap

SPLIT="${1:-32k}"
QUERY_LIMIT="${2:-50}"
CHUNK_SIZES="${CHUNK_SIZES:-256 512 1024}"
OVERLAP_RATIO="${OVERLAP_RATIO:-0.125}"
TURNS_PER_CHUNK="${TURNS_PER_CHUNK:-4}"
OVERLAP_TURNS="${OVERLAP_TURNS:-1}"

WORK_DIR="${WORK_DIR:-/workspace}"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"
CHITTA_DIR="$WORK_DIR/chitta"
SWEEP_DIR="$AMB_DIR/outputs/personamem/chitta-mcp-sweep"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export OPENBLAS_NUM_THREADS=4
export TURNS_PER_CHUNK OVERLAP_TURNS

mkdir -p "$SWEEP_DIR"

pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start

cd "$AMB_DIR"

if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi

_restart_chitta() {
    pkill -f "chitta-rs --http" || true
    sleep 2
    cd "$CHITTA_DIR"
    ./target/release/chitta-rs --http --auth-token-file ~/.config/chitta/bearer-token.txt &
    sleep 5
    cd "$AMB_DIR"
}

echo "=== PersonaMem $SPLIT sweep ==="
echo "Provider: chitta-mcp (chitta-rs HTTP)"
echo "Chunk sizes: $CHUNK_SIZES"
echo "Overlap ratio: $OVERLAP_RATIO"
echo "Turns/chunk: $TURNS_PER_CHUNK   Overlap turns: $OVERLAP_TURNS"
echo "Query limit per config: $QUERY_LIMIT"
echo ""

for CHUNK in $CHUNK_SIZES; do
    OVERLAP=$(python3 -c "print(int($CHUNK * $OVERLAP_RATIO))")
    TAG="cs${CHUNK}_ov${OVERLAP}"
    OUT_DIR="$SWEEP_DIR/$TAG"
    mkdir -p "$OUT_DIR"

    echo "------------------------------------------------------------"
    echo "[$TAG] chunk_size=$CHUNK overlap=$OVERLAP"
    echo "------------------------------------------------------------"

    echo "[$TAG] resetting DB..."
    su - postgres -c "psql -c 'DROP DATABASE IF EXISTS chitta_beam;'"
    su - postgres -c "psql -c 'CREATE DATABASE chitta_beam OWNER chitta;'"
    su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"

    _restart_chitta

    export CHITTA_CHUNK_SIZE="$CHUNK"
    export CHITTA_CHUNK_OVERLAP="$OVERLAP"
    export CHITTA_TURNS_PER_CHUNK="$TURNS_PER_CHUNK"
    export CHITTA_OVERLAP_TURNS="$OVERLAP_TURNS"

    START=$(date +%s)
    uv run omb run \
        --dataset personamem \
        --split "$SPLIT" \
        --memory chitta-mcp \
        --llm gemini \
        --query-limit "$QUERY_LIMIT" \
        --name "chitta-mcp-sweep-$TAG"
    END=$(date +%s)
    ELAPSED=$(( END - START ))
    echo "[$TAG] wall time: ${ELAPSED}s"

    SRC="$AMB_DIR/outputs/personamem/chitta-mcp-sweep-$TAG/rag/${SPLIT}.json.gz"
    if [ -f "$SRC" ]; then
        cp "$SRC" "$OUT_DIR/${SPLIT}.json.gz"
    else
        echo "[$TAG] WARN: no result at $SRC"
    fi
done

echo ""
echo "============================================================"
echo "SWEEP SUMMARY (PersonaMem $SPLIT, $QUERY_LIMIT queries each)"
echo "============================================================"
python3 <<PY
import gzip, json, os, glob
sweep_dir = "$SWEEP_DIR"
rows = []
for tag_dir in sorted(glob.glob(os.path.join(sweep_dir, "cs*_ov*"))):
    tag = os.path.basename(tag_dir)
    path = os.path.join(tag_dir, "${SPLIT}.json.gz")
    if not os.path.exists(path):
        rows.append((tag, None, None, None, None))
        continue
    with gzip.open(path, "rt") as f:
        d = json.load(f)
    rows.append((
        tag,
        d.get("accuracy"),
        d.get("avg_context_tokens"),
        d.get("ingestion_time_ms"),
        d.get("avg_retrieve_time_ms"),
    ))
print(f"{'config':<20} {'accuracy':>10} {'avg_ctx_tok':>12} {'ingest_s':>10} {'retr_ms':>10}")
print("-" * 68)
for tag, acc, ctx, ing, retr in rows:
    if acc is None:
        print(f"{tag:<20} {'MISSING':>10}")
        continue
    print(
        f"{tag:<20} "
        f"{acc:>10.4f} "
        f"{(ctx or 0):>12.0f} "
        f"{(ing or 0)/1000:>10.1f} "
        f"{(retr or 0):>10.1f}"
    )
PY

echo ""
echo "Result blobs: $SWEEP_DIR/cs*/${SPLIT}.json.gz"
