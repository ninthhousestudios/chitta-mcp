#!/usr/bin/env bash
set -euo pipefail

# RunPod AMB benchmark setup script for chitta-rs.
#
# Recommended pod:
#   Image: runpod/pytorch:2.8.0-py3.11-cuda12.8.1-cudnn-devel-ubuntu22.04
#   GPU:   L40S or L4 (embedding via chitta-rs ONNX)
#   vCPU:  12+
#   Disk:  80GB+
#   RAM:   32GB+
#
# Usage:
#   export GEMINI_API_KEY="your-key"
#   bash setup.sh

echo "=== RunPod AMB Benchmark Setup (chitta-rs) ==="

# ── OpenBLAS thread limit (must be before any scipy/numpy import) ────
export OPENBLAS_NUM_THREADS=4
echo "OPENBLAS_NUM_THREADS=4" >> /etc/environment

# ── System deps ─────────��───────────────────────────���────────────────
apt-get update -qq
apt-get install -y -qq postgresql postgresql-contrib postgresql-server-dev-all \
    build-essential git curl pkg-config libssl-dev

# ── Rust toolchain ───────────────────────────────────────────────────
if ! command -v cargo &> /dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
export PATH="$HOME/.cargo/bin:$PATH"

# ── pgvector extension ───────────────────────────────────────────────
PG_SHAREDIR=$(pg_config --sharedir 2>/dev/null || echo "")
if [ -n "$PG_SHAREDIR" ] && [ -f "$PG_SHAREDIR/extension/vector.control" ]; then
    echo "pgvector already installed — skipping build."
else
    echo "Installing pgvector..."
    rm -rf /tmp/pgvector
    cd /tmp
    git clone --branch v0.8.0 --depth 1 https://github.com/pgvector/pgvector.git
    cd pgvector
    make && make install
    cd /
fi

# ���─ Start postgres ────────────────────────────────────────────��──────
pg_isready -q 2>/dev/null || pg_ctlcluster $(pg_lsclusters -h | awk '{print $1, $2}') start
sleep 2

# ── Postgres setup ���──────────────────────────────────────────────────
su - postgres -c "psql -c \"ALTER USER postgres PASSWORD 'postgres';\""

su - postgres -c "psql -tc \"SELECT 1 FROM pg_roles WHERE rolname='chitta'\"" | grep -q 1 || \
    su - postgres -c "psql -c \"CREATE ROLE chitta WITH LOGIN PASSWORD 'chitta';\""

su - postgres -c "psql -tc \"SELECT 1 FROM pg_database WHERE datname='chitta_beam'\"" | grep -q 1 || \
    su - postgres -c "createdb -O chitta chitta_beam"

su - postgres -c "psql -d chitta_beam -c 'CREATE EXTENSION IF NOT EXISTS vector;'"
su - postgres -c "psql -d chitta_beam -c 'GRANT ALL ON DATABASE chitta_beam TO chitta;'"
su - postgres -c "psql -c \"ALTER SYSTEM SET max_wal_size = '2GB';\""
su - postgres -c "psql -c 'SELECT pg_reload_conf();'"

echo "Postgres ready: chitta_beam with pgvector"

# ── uv ────────────���──────────────────────────────────────────────────
pip install uv
export PATH="$HOME/.local/bin:$PATH"

# ── Clone repos ──���───────────────────────────────────────────────────
WORK_DIR="${WORK_DIR:-/workspace}"
mkdir -p "$WORK_DIR"

CHITTA_DIR="$WORK_DIR/chitta"
AMB_DIR="$WORK_DIR/agent-memory-benchmark"

if [ ! -d "$CHITTA_DIR" ]; then
    git clone https://gitlab.com/ninthhouse/chitta-mcp.git "$CHITTA_DIR"
    cd "$CHITTA_DIR"
else
    echo "chitta repo exists, pulling latest..."
    cd "$CHITTA_DIR" && git pull
fi

if [ ! -d "$AMB_DIR" ]; then
    git clone https://github.com/ninthhousestudios/agent-memory-benchmark.git "$AMB_DIR"
else
    echo "AMB repo exists, pulling latest..."
    cd "$AMB_DIR" && git pull
fi

# ── Download BGE-M3 model ───────────────────────────────────────────
pip install huggingface-hub
MODEL_DIR="$HOME/.cache/chitta/bge-m3-onnx"
mkdir -p "$MODEL_DIR"
if [ ! -f "$MODEL_DIR/bge_m3_model.onnx" ]; then
    echo "Downloading BGE-M3 dense+sparse ONNX model from HuggingFace..."
    uv run hf download prometheus-en-croute/bge-m3-dense-sparse \
        --local-dir "$MODEL_DIR"
    # The repo stores ONNX files inside a directory named "tokenizer.json/"
    if [ -f "$MODEL_DIR/tokenizer.json/bge_m3_model.onnx" ]; then
        mv "$MODEL_DIR/tokenizer.json/bge_m3_model.onnx" "$MODEL_DIR/bge_m3_model.onnx"
        mv "$MODEL_DIR/tokenizer.json/bge_m3_model.onnx_data" "$MODEL_DIR/bge_m3_model.onnx_data"
        rm -rf "$MODEL_DIR/tokenizer.json" 2>/dev/null || true
    fi
else
    echo "BGE-M3 ONNX model already cached."
fi
if [ ! -f "$MODEL_DIR/tokenizer.json" ]; then
    echo "Downloading BGE-M3 tokenizer.json from BAAI/bge-m3..."
    uv run hf download BAAI/bge-m3 tokenizer.json \
        --local-dir "$MODEL_DIR"
else
    echo "BGE-M3 tokenizer already cached."
fi

# ── Find ONNX runtime library ───────────────────────────────────────
ORT_LIB=$(find / -name "libonnxruntime.so*" -not -path "*/proc/*" 2>/dev/null | head -1 || true)
if [ -z "$ORT_LIB" ]; then
    echo "Installing onnxruntime-gpu for ORT_DYLIB_PATH..."
    pip install onnxruntime-gpu
    ORT_LIB=$(find / -name "libonnxruntime.so*" -not -path "*/proc/*" 2>/dev/null | head -1)
fi
echo "ORT_DYLIB_PATH=$ORT_LIB"

# ��─ Build chitta-rs ──────────────────────────────────────────────────
cd "$CHITTA_DIR"

cat > .env <<EOF
DATABASE_URL=postgresql://chitta:chitta@localhost/chitta_beam
ORT_DYLIB_PATH=$ORT_LIB
EOF

echo "Building chitta-rs (release)..."
cargo build --release
echo "chitta-rs built: $CHITTA_DIR/target/release/chitta-rs"

# ── Generate bearer token ───────────────────────────────────────────
TOKEN_DIR="$HOME/.config/chitta"
mkdir -p "$TOKEN_DIR"
if [ ! -f "$TOKEN_DIR/bearer-token.txt" ]; then
    python3 -c "import secrets; print(secrets.token_hex(32))" > "$TOKEN_DIR/bearer-token.txt"
fi
BEARER_TOKEN=$(cat "$TOKEN_DIR/bearer-token.txt")

# ── Install AMB deps ────────────────────────────────────────────────
cd "$AMB_DIR"
uv sync
uv pip install httpx

# ── Write .env for AMB ──────────────────────────────────────────────
cat > "$AMB_DIR/.env" <<EOF
GEMINI_API_KEY=${GEMINI_API_KEY:?Set GEMINI_API_KEY before running setup}
OMB_ANSWER_LLM=gemini
CHITTA_RS_URL=http://127.0.0.1:3100/mcp
CHITTA_BEARER_TOKEN=$BEARER_TOKEN
OPENBLAS_NUM_THREADS=4
EOF

echo ""
echo "=== Setup complete ==="
echo ""
echo "Start chitta-rs before running benchmarks:"
echo "  cd $CHITTA_DIR"
echo "  ./target/release/chitta-rs --http --auth-token-file $TOKEN_DIR/bearer-token.txt &"
echo ""
echo "Run benchmarks with:"
echo "  cd $AMB_DIR"
echo "  bash bench/runpod/run-beam.sh 100k"
