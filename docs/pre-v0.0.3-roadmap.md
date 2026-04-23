# pre-v0.0.3 roadmap

chitta-rs v0.0.2 baselines: PersonaMem 32k **64.3%**, BEAM 100k **66.6%**.

No features are pre-committed. The numbers decide what goes into v0.0.3.

## North star update

chitta is a **cognitive confluence system** — the place where human knowledge and
agent capability meet. Agent-native and human-native, but separable.
Close the laptop and chitta waits. The agent is the primary interface
consumer; the human is the primary knowledge owner. The combination
produces something neither could alone.

Three layers of knowledge, each with a different home:

| Layer | Description | Home |
|---|---|---|
| Personal/experiential | Session notes, patterns, craft insights | Chitta |
| Reference/domain | Textbook definitions, sourced claims | Separate reference DB (future) |
| Structural/framework | Deep domain grammar, relational knowledge | Fine-tuned model (future) |

v0.0.3 focuses on retrieval quality — the foundation all three layers need.

## Experiment plan

Three rounds. Each round's results inform whether to proceed with the next.

### Round 1: adapter-only sweeps (no server changes)

These cost nothing but RunPod time. They tell us how much headroom exists
without touching the server.

**1a. k sweep** — `CHITTA_K=10,20,30,40` on both benchmarks.
Targets multi_session_reasoning (42.5%), suggest_new_ideas (25.8%).

**1b. Chunk-size sweep** — `CHITTA_CHUNK_SIZE=256,512,1024` on PersonaMem.
Targets PersonaMem overall accuracy.

**1c. Combined** — best chunk size from 1b x best k from 1a.
Tests interaction effects.

**Decision gate:** If k=30+ gives a meaningful lift (3%+), context assembly
and diversity matter more than retrieval precision — pushes toward reranking.
If k barely helps, the problem is retrieval quality itself — pushes toward
recency weighting and graph.

Script: `bench/runpod/run-v003-round1.sh`

### Round 2: targeted server-side changes (small code)

The two highest-leverage changes identified in both benchmark analyses.
Test independently, then combined.

**2a. Recency-weighted scoring** — modify the SQL ranking query:
`score = cosine * (1 + w * recency_factor)`. Sweep `w` over 0.05, 0.1, 0.2, 0.3.
Targets knowledge_update (45%), preference_evolution (57.6%).

**2b. Recency + best k from Round 1** — combined.

Implementation: SQL modification using `event_time` (already stored).
No schema change needed.

**Decision gate:** If recency weighting lifts knowledge_update by 15%+ and
preference_evolution by 10%+, it goes into v0.0.3 core. The winning weight
`w` tells us how much temporal signal matters relative to semantic similarity.

### Round 3: pick one moderate change

Based on Round 1 and 2 results, pick **one** of:

**3a. Cross-encoder reranking** — pick this if high-k helped in Round 1
(good candidates exist but ranking is wrong). ONNX reranker in existing
session pool. Targets information_extraction, general precision.

**3b. Multi-query retrieval** — pick this if high-k didn't help (single
queries don't find enough relevant memories). New server tool or adapter
logic. Targets multi_session_reasoning, suggest_new_ideas.

### What stays out

- **Entity/graph extraction** — high ceiling but too large for a test round.
  Benchmark results from Rounds 1-3 will tell us whether we need structural
  changes or can reach 75%+ with retrieval tuning alone.
- **BGE-M3 sparse weights** — both benchmark analyses are skeptical this
  moves the needle on the actual failure modes. Low priority.
- **Reference DB** — right architecture, wrong time. Aion concern, not a
  chitta benchmark concern.
- **Fine-tuning** — same. Important for the vision, not for v0.0.3.

## Success criteria

v0.0.3 ships when we have:
1. At least one benchmark at 75%+ accuracy
2. Clear understanding of which retrieval signals matter (temporal, diversity, precision)
3. No regression on currently-strong categories (preference_following 95%, summarization 95%)
