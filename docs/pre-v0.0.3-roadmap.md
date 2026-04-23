# pre-v0.0.3 roadmap

chitta-rs v0.0.2 baselines: PersonaMem 32k **64.3%**, BEAM 100k **66.6%**.

## What Round 1-2 experiments showed

Three rounds of retrieval-tuning experiments (k sweep, chunk-size sweep,
recency-weighted scoring) all moved the needle by <1pp in aggregate. The
single-pass dense cosine pipeline is at its ceiling for these benchmarks.

Key findings:
- **k sweep** (<3% lift): more results don't help — the right memories
  aren't being found, not ranked poorly.
- **Chunk-size sweep** (60.1–63.0%): small deltas, not the lever.
- **Recency weighting** (w=0.05 best): +0.51pp on PersonaMem, -0.75pp on
  BEAM. Wash. Helps temporal categories, hurts precision categories.
  Decision gate (knowledge_update +15%, preference_evolution +10%) not met.

Results: `bench/results/pre-v0003-tests/`.

## Failure modes (unchanged)

| Root cause | Categories affected | Accuracy |
|---|---|---|
| Single query can't capture multi-faceted needs | suggest_new_ideas, multi_session_reasoning | 26–50% |
| No temporal/factual structure | knowledge_update, preference_evolution | 45–58% |
| Precision gaps | information_extraction | 50–63% |

Context token counts are identical for correct and wrong answers in both
benchmarks. This is a context *quality* problem, not quantity.

## Next steps

### 1. Hybrid retrieval experiment (RunPod)

Test whether adding term-matching signals lifts accuracy beyond what
parameter tuning couldn't. BGE-M3 already computes sparse weights in
`src/embedding.rs` — they're currently discarded.

**Test matrix (PersonaMem + BEAM):**
1. Dense only (current baseline, already have numbers)
2. Dense + sparse (enable BGE-M3 sparse output, RRF merge)
3. Dense + FTS (add tsvector column + GIN index, RRF merge)
4. Dense + sparse + FTS (three-way RRF, Python chitta's approach)

Python chitta had RRF and also scored 64% on PersonaMem, so this may not
move the needle either. That's a valid outcome — it tells us the problem
is structural, not retrieval-pipeline.

If any config crosses 70%, multi-query retrieval is the natural follow-up
(targets suggest_new_ideas 26% and multi_session_reasoning 40%).

### 2. Agent-native quality (ships regardless of benchmark results)

These make chitta better for real agents independent of benchmark scores:

- **Error message quality** — every error includes: what the agent did,
  why it was rejected, what to do next with a concrete example. Audit all
  error paths.
- **Corpus health diagnostics** — read-only tool returning counts,
  coverage, duplicate pairs above threshold, orphan detection. Useful now,
  grows richer as more features land.
- **Recency weighting code** — already implemented, ships with w=0.0
  default (no behavior change). Available for users who want it.

### 3. Direction after experiments

If hybrid retrieval crosses 70%: v0.0.3 = hybrid pipeline + agent-native
quality. Add multi-query retrieval if time allows.

If hybrid retrieval doesn't help: v0.0.3 = agent-native quality + graph
foundation (entity extraction, supersede links, async enrichment queue).
The benchmark failure modes need structural changes, not retrieval tuning.

## What stays out

- **Cross-encoder reranking** — k sweep showed ranking isn't the issue.
- **Entity graph** — unless hybrid retrieval fails and we pivot.
- **FSRS / learning layer** — needs graph foundation first.
- **Compiled wiki / synthesis** — Phase 5 territory.
