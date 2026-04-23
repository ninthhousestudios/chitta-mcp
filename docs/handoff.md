# Handoff

## Pick up

- **Revised pre-v0.0.3 roadmap is written.** Round 1-2 experiments (k, chunk size, recency weight) showed parameter tuning is at ceiling. New plan: hybrid retrieval experiment + agent-native quality.
- **Hybrid retrieval experiment needs implementation.** Enable BGE-M3 sparse weights (currently discarded in `src/embedding.rs`), add FTS/tsvector, build RRF merge. Test matrix: dense-only (have baseline), dense+sparse, dense+FTS, dense+sparse+FTS. Need a RunPod script.
- **BEAM script ready but not yet used.** `bench/runpod/run-v003-beam.sh` runs BEAM at w=0.0 and w=0.05. Written this session but the BEAM runs were done with a different script/naming (`100k-rw000.json`, `100k-rw0005.json`).

## Context

- Round 1-2 results in `bench/results/pre-v0003-tests/`. PersonaMem rw sweep: w=0.05 best at 64.18%, control 63.67%. BEAM: control 65.38%, w=0.05 64.63%. Recency weighting is a wash.
- Per-category breakdown: suggest_new_ideas (26%), multi_session_reasoning (40%), knowledge_update (45%) are the real drags. These need structural fixes, not tuning.
- Python chitta had RRF and scored 64% on PersonaMem — so hybrid retrieval may also not help. That's a valid outcome that would point toward graph/entity work for v0.0.3.
- Recency weighting code is in chitta-rs (config.rs, db.rs, mcp.rs, search.rs) with w=0.0 default. No behavior change, ships as-is.

## Uncommitted changes

- `docs/pre-v0.0.3-roadmap.md` — new roadmap
- `docs/index.md` — updated manifest
- `docs/archived/pre-v0.0.3-roadmap-first-try.md` — Josh moved the old roadmap here
- `bench/runpod/run-v003-beam.sh` — beam-only script (new)
- Recency weighting Rust code from prior session (config.rs, db.rs, mcp.rs, main.rs, tools/search.rs)
- Benchmark result files in `bench/results/pre-v0003-tests/`
