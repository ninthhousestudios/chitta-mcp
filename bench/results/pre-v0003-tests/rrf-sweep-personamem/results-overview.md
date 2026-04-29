# RRF sweep results: PersonaMem retrieval-only

Date: 2026-04-24
Dataset: PersonaMem 32k (589 queries, 195 docs)
Eval: retrieval-only (no LLM) via `bench/retrieval-eval.py`
Retrieval config: k=20, chunk_size=512, overlap=64

## Results

All configs achieve 100% hit rate and MRR=1.0 (first result is always a gold doc).
The differentiator is **recall@k** — how many of the gold docs land in the top-k.

| Config | Legs | rrf_k | Recall@20 | Gold overlap | Retrieve (ms) | Ctx tokens |
|--------|------|-------|-----------|--------------|---------------|------------|
| dense-only | dense | - | 93.9% | 84.5% | 42.2 | 16,028 |
| dense-fts | dense+fts | 60 | 93.9% | 84.5% | 42.2 | 16,027 |
| dense-sparse | dense+sparse | 60 | 96.8% | 85.0% | 42.6 | 16,692 |
| dense-fts-sparse | dense+fts+sparse | 60 | 96.8% | 85.0% | 42.9 | 16,692 |
| **rrf-k20** | dense+fts+sparse | 20 | **97.0%** | 84.9% | 43.8 | 16,620 |
| rrf-k60 | dense+fts+sparse | 60 | 96.8% | 85.0% | 43.3 | 16,691 |
| rrf-k120 | dense+fts+sparse | 120 | 96.6% | 84.9% | 43.3 | 16,711 |

## What the data says

**Sparse retrieval is the win.** Adding sparse (BGE-M3 sparse weights) to dense bumps recall from 93.9% to 96.8% (+2.9pp). This is the only change that moves the needle meaningfully.

**FTS adds nothing.** dense-fts is identical to dense-only. dense-fts-sparse is identical to dense-sparse. Full-text search contributes zero incremental recall on PersonaMem. Likely because the BGE-M3 sparse weights already capture the lexical signal that FTS would provide.

**rrf_k barely matters.** k=20, k=60, k=120 all land within 0.4pp of each other (97.0%, 96.8%, 96.6%). The RRF smoothing constant doesn't affect this dataset much.

**No latency cost.** All configs retrieve in ~42-44ms. RRF fusion is essentially free at this scale (195 docs).

**Gold term overlap is flat.** Overlap stays at 84.5-85.0% regardless of config — retrieval strategy changes which docs surface but not how much answer-relevant vocabulary they contain.

## Per-category recall (dense-only vs dense+sparse)

| Category | n | Dense | +Sparse | Delta |
|----------|---|-------|---------|-------|
| generalizing_to_new_scenarios | 57 | 98.2% | 99.8% | +1.6pp |
| provide_preference_aligned_rec | 55 | 93.1% | 96.8% | +3.7pp |
| recall_user_shared_facts | 129 | 96.3% | 97.5% | +1.2pp |
| recalling_facts_mentioned_by_the_user | 17 | 94.7% | 97.7% | +3.0pp |
| recalling_the_reasons_behind_previous_updates | 99 | 92.9% | 95.6% | +2.7pp |
| suggest_new_ideas | 93 | 90.8% | 96.0% | +5.2pp |
| track_full_preference_evolution | 139 | 92.8% | 96.0% | +3.2pp |

Sparse helps most on `suggest_new_ideas` (+5.2pp) and `provide_preference_aligned_rec` (+3.7pp) — categories where lexical matching of specific preferences/facts supplements dense semantic similarity.

## Comparison to prior RAG results

Previous RAG-mode sweeps (with gemini-2.5-flash-lite for answer+judge) on the same dataset scored 60-64% end-to-end accuracy. Retrieval recall is 94-97% — substantially higher. This gap means retrieval is not the primary bottleneck for end-to-end accuracy on PersonaMem. The LLM's ability to synthesize answers from retrieved context is the limiting factor.

## What to try next

### 1. BEAM RRF sweep (in progress)
Same retrieval-only sweep on BEAM to see if sparse/RRF has a different effect on a different benchmark. BEAM has per-conversation isolation and different question types — the sparse signal may matter more or less.

### 2. Stronger answer LLM
The 94-97% retrieval recall vs 60-64% RAG accuracy gap is ~30pp. That gap is the answer LLM struggling with the retrieved context, not retrieval failing to find the right docs. Testing with a stronger model (gemini-2.5-flash or pro) on the best retrieval config would quantify how much of the ceiling is LLM-limited vs retrieval-limited. Most cost-effective single experiment to run.

### 3. Context presentation
The LLM gets ~16k tokens of context. How that context is formatted, ordered, or summarized before the LLM sees it could matter. Options:
- Rerank retrieved chunks by relevance before presenting
- Truncate to top-N most relevant chunks (less noise)
- Add metadata (timestamps, source doc IDs) to help the LLM reason about temporal questions

### 4. Structured extraction
For categories like `track_full_preference_evolution` and `suggest_new_ideas`, raw chunk retrieval surfaces the right text but the LLM must piece together facts scattered across chunks. An entity-attribute store (explicit facts with timestamps) could help these categories without relying on LLM synthesis.

### 5. Query decomposition / multi-hop
Some questions implicitly require multiple retrieval passes (e.g., "how did X change over time?"). An agentic retrieve-inspect-retrieve loop could help, though the high single-pass recall (97%) suggests this is less urgent than fixing the LLM gap.

## Recommendation

Enable `rrf_sparse` as the production default — it's a free +2.9pp recall with no latency cost. Drop FTS (no value). Use rrf_k=60 (default). Then invest in the answer LLM experiment to understand the 30pp retrieval-to-RAG gap before pursuing architectural changes.
