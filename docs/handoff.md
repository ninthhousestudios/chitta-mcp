# Handoff

## Pick up

- **Run retrieval-only sweeps on RunPod.** RRF implementation is complete (commit c06f3aa). Build the release binary (`cargo build --release`) and run:
  ```bash
  bash bench/runpod/run-retrieval-sweep.sh personamem 32k
  ```
  Seven configs: dense-only, dense-fts, dense-sparse, dense-fts-sparse, rrf-k20, rrf-k60, rrf-k120. Compare MRR/recall/precision across combos.

- **Backfill existing databases.** If running against a DB with pre-existing data (not fresh sweep), run the backfill first:
  ```bash
  chitta-rs backfill
  ```
  This populates `sparse_embedding` for rows that pre-date migration 0004. The tsvector column is auto-maintained by Postgres (GENERATED ALWAYS).

- **After sweep results:** If hybrid retrieval moves the needle, pick the best config combo and make it the default. If not, the per-category drags (suggest_new_ideas 26%, multi_session_reasoning 40%, knowledge_update 45%) point toward graph/entity work as the next lever.

## Context

- RRF implementation: plan at `.agents/plans/2026-04-23-rrf-hybrid-retrieval.md`, pre-mortem at `.agents/council/2026-04-23-pre-mortem-rrf-hybrid-retrieval.md`.
- ONNX `sparse_weights` shape verified: `[batch, seq_len, 1]`. Extraction zips token IDs with per-position weights, thresholds at `CHITTA_SPARSE_THRESHOLD` (default 0.01), dedupes by max weight per token ID.
- Zero-overhead path: when `CHITTA_RRF_FTS=false` and `CHITTA_RRF_SPARSE=false` (defaults), behavior is identical to v0.0.2 — no regression risk.
- Clippy `too_many_arguments` warnings on search_hybrid (13 args), search::handle (10 args), ChittaServer::new (10 args). Could group RRF params into a struct if this bothers — plan chose individual flags for sweep composability.
- Round 1-2 dense cosine + recency tuning at ceiling (~64% PersonaMem). Python chitta's RRF scored 64% — hybrid may not move the number, but systematic testing will confirm.
