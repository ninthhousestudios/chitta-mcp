# astrobench — how to run

Retrieval benchmark for chitta-rs using the astrology vault corpus. Measures dense-only vs hybrid (dense+sparse, dense+sparse+FTS) on four query slices targeting different retrieval challenges.

See `docs/plans/astrology-benchmark-plan.md` for design rationale and decision matrix.

## Prerequisites

- Postgres with pgvector extension installed
- Python 3.11+
- uv
- BGE-M3 ONNX model at `~/.cache/chitta/bge-m3-onnx/` (same model chitta-rs uses)
- Vault directories: `~/vault/astro/`, `~/vault/iching/`, `~/vault/cards/`

## Setup (one-time)

All commands run from `bench/astrobench/`.

```bash
cd bench/astrobench
```

### 1. Install dependencies

```bash
uv sync
```

### 2. Provision the database

Creates `chitta_astrobench` and applies all chitta-rs migrations.

```bash
bash provision.sh
```

To use a different database name:

```bash
bash provision.sh my_custom_db_name
```

### 3. Ingest the corpus

Chunks all `.md` files from the vault, computes dense+sparse embeddings via BGE-M3 ONNX (CPU), and inserts directly into Postgres. Idempotent — safe to rerun.

```bash
uv run python ingest.py
```

Takes ~35-40 minutes on CPU for all three profiles (~1600 chunks total). Progress prints every 50 files.

Options:

```bash
# ingest only astro (skip distractors)
uv run python ingest.py --profiles astro

# custom vault location or DB
uv run python ingest.py --vault /path/to/vault --db "postgresql://user:pass@host/dbname"

# different chunking (default: 512 tokens, 64 overlap)
uv run python ingest.py --chunk-size 256 --chunk-overlap 32
```

### 4. Snapshot (optional)

Save the DB so you can restore without re-embedding:

```bash
pg_dump -Fc chitta_astrobench > ../../bench/datasets/astrobench/snapshot-$(date +%Y%m%d).dump
```

Restore:

```bash
pg_restore -d chitta_astrobench snapshot-20260427.dump
```

## Authoring queries

After ingest, author queries following `docs/astrobench-query-authoring.md`. Place JSONL files in `bench/datasets/astrobench/queries/`:

```
slice-a.jsonl   ~30 queries  Sanskrit / jargon
slice-b.jsonl   ~20 queries  exact identifiers
slice-c.jsonl   ~15 queries  natural language
slice-d.jsonl   ~15 queries  multi-hop / relational
```

### Slice B helper

For slice B, find unique tokens in the corpus:

```bash
uv run python find-unique-tokens.py
```

Outputs hapax candidates (tokens appearing in exactly one chunk) with context. Pick ~20 meaningful identifiers and write questions around them.

Options:

```bash
# longer tokens only, more candidates
uv run python find-unique-tokens.py --min-length 6 --limit 500
```

Results are also written to `hapax-candidates.txt` for easier browsing.

### Resolving gold chunk IDs

Each query needs `gold_chunk_ids` — the chitta memory UUIDs of chunks that answer the query. To find them, connect to the bench DB and search:

```bash
psql chitta_astrobench -c "
  SELECT id, left(content, 200)
  FROM memories
  WHERE profile = 'astro'
    AND content ILIKE '%your search term%'
  ORDER BY record_time
  LIMIT 10
"
```

Or run a chitta-rs instance pointed at the bench DB and use `search_memories`:

```bash
DATABASE_URL=postgresql://josh:ogham@localhost/chitta_astrobench cargo run -- stdio
```

## Running the evaluation

The eval script reimplements chitta-rs's retrieval pipeline (dense ANN, FTS, sparse dot-product, RRF fusion) directly against the database. No running server needed.

### Single config

```bash
uv run python eval.py --config dense-only
uv run python eval.py --config dense-sparse
uv run python eval.py --config dense-sparse-fts
```

### Sweep all configs

```bash
uv run python eval.py --sweep
```

### Options

```bash
# evaluate only specific slices
uv run python eval.py --sweep --slices a b

# custom k values (default: 5 10 20)
uv run python eval.py --sweep --k 5 10 20 50

# custom output directory
uv run python eval.py --sweep -o results/run-001
```

### Output

Each config produces a JSON file in `outputs/` (or your `--output-dir`):

```
outputs/dense-only.json
outputs/dense-sparse.json
outputs/dense-sparse-fts.json
```

Console output includes a per-slice breakdown:

```
======================================================================
  config: dense-sparse  |  legs: dense+sparse  |  rrf_k=60  |  80 queries
======================================================================
  recall@5:   72.3%
  recall@10:  81.5%
  recall@20:  89.2%
  hit rate:   93.1%
  MRR:        0.6842
  latency:    45.2ms

  Per-slice breakdown:
    slice       n  r@5     r@10    r@20    hit     mrr     ms
    a          30  85.0%   91.2%   96.8%   100.0%  0.7234  42.1
    b          20  90.0%   95.0%   100.0%  100.0%  0.8500  38.7
    c          15  65.0%   72.0%   80.0%   86.7%   0.5432  47.3
    d          15  40.0%   55.0%   62.0%   73.3%   0.4102  51.8
```

(Numbers above are illustrative, not actual results.)

## Interpreting results

See the decision matrix in `docs/plans/astrology-benchmark-plan.md`. The key thresholds:

- **Slice A/B**: sparse adds >5pp recall@20 → hybrid wins on jargon/identifiers
- **Slice C**: sparse hurts >2pp → hybrid shouldn't be unconditional
- **Slice D**: recall@20 >70% → retrieval not the bottleneck; <60% → KG justified

## File overview

| File | Role |
|---|---|
| `pyproject.toml` | Python deps (onnxruntime, tokenizers, psycopg, pgvector) |
| `provision.sh` | Create DB + run migrations |
| `embedder.py` | BGE-M3 ONNX embedder (shared by ingest + eval) |
| `ingest.py` | Chunk vault .md files → embed → insert into Postgres |
| `find-unique-tokens.py` | Hapax token finder for slice B authoring |
| `eval.py` | Retrieval evaluation with per-slice recall@k reporting |
