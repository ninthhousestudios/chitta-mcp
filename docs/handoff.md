# Handoff

## Pick up

- **Round 2 sweep is ready to run.** Code compiled, script at `bench/runpod/run-v003-round2.sh`. Spin up RunPod pod, `git pull`, `cargo build --release`, then `bash run-v003-round2.sh`. Quick test first with `bash run-v003-round2.sh 50`.
- Recency weighting defaults to `0.0` (off), so the live Chitta server is unaffected.

## Context

- Round 1 results are in `bench/results/pre-v0003-tests/personamem/`. k sweep showed <3% lift (decision gate says: focus on retrieval quality, not reranking).
- Round 2 sweeps `CHITTA_RECENCY_WEIGHT` over 0.0, 0.05, 0.1, 0.2, 0.3 on PersonaMem (k=20), then validates best w on BEAM.
- AMB answer/judge LLMs are set to Gemini in both round1 and round2 scripts. For official leaderboard runs later, remove `OMB_ANSWER_LLM` and `OMB_JUDGE_LLM` exports to restore Groq defaults.

## Uncommitted changes

- Rust: recency weighting in config.rs, db.rs, mcp.rs, main.rs, tools/search.rs + test fixes
- Scripts: run-v003-round1.sh (gemini fix), run-v003-round2.sh (new)
- Infra: .sessions/ dir, sync.sh, .gitignore update, CLAUDE.md session artifact policy
