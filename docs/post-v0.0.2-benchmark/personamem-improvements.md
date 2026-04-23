# PersonaMem benchmark improvements

Baseline: chitta-rs v0.0.2, PersonaMem 32k split, **64.3%** (379/589).

## Failure analysis

Category breakdown reveals where the 210 wrong answers come from:

| Question type | Accuracy | Notes |
|---|---|---|
| generalizing_to_new_scenarios | 87.7% (50/57) | Strong — dense similarity handles this well |
| recalling_reasons_behind_updates | 83.8% (83/99) | Strong |
| recall_user_shared_facts | 71.3% (92/129) | Decent |
| recalling_facts_mentioned_by_user | 70.6% (12/17) | Small sample but decent |
| provide_preference_aligned_recs | 69.1% (38/55) | Needs broader preference context |
| track_full_preference_evolution | 57.6% (80/139) | Temporal awareness gap |
| suggest_new_ideas | 25.8% (24/93) | **Worst category by far** |

Wrong answers skew toward "a" (94/210), suggesting the LLM defaults to the
first option when retrieved context doesn't contain the right signal.

Context token counts are nearly identical for correct (11,876) and wrong
(11,746) answers — this is a context *quality* problem, not quantity.

## Improvement ideas

### Tier 1: low-hanging fruit (no architecture changes)

**1. Activate BGE-M3 sparse weights**

BGE-M3 already computes sparse term weights in `embedding.rs` — they're
discarded. Store them in a sparse vector column (pgvector supports
`sparsevec`), run a second ANN pass on sparse similarity, merge via RRF.
This is flagged in `innovation-potentials.md` as "essentially free."

However — Python chitta had RRF and also got 64%. So sparse might not help
PersonaMem specifically. Worth testing to confirm, but don't expect a big
jump here alone.

**2. Tune the adapter's chunking strategy**

The benchmark adapter controls `CHITTA_CHUNK_SIZE` and
`CHITTA_CHUNK_OVERLAP`. The sweep script tests 256/512/1024 tokens. The
current run used defaults — the right chunk size for persona conversations
may differ from general-purpose text. Smaller chunks improve precision
(find the exact preference statement), larger chunks preserve context
(understand preference evolution).

Run the sweep: `bash bench/runpod/run-personamem-sweep.sh`.

**3. Increase k**

Default k=10 may miss relevant context. For PersonaMem's multiple-choice
format, casting a wider net (k=20 or k=30) and letting the LLM sort through
more context could help, especially for preference-evolution questions that
span many memories.

**4. Query augmentation in the adapter**

PersonaMem queries include the user's name. The adapter could prepend
"preferences and interests of [user]" or extract key terms from the
question to improve embedding similarity. This is an adapter-side change,
not a chitta-rs change.

### Tier 2: moderate effort (targeted retrieval changes)

**5. Temporal signal in ranking**

`track_full_preference_evolution` (57.6%) fails because pure cosine
similarity doesn't know which memories are newer. A preference stated
recently should outrank one from months ago when the question is about
current preferences.

Approach: add recency decay as a soft boost on the similarity score. The
bi-temporal model already stores `event_time`. A scoring function like
`cosine * (1 + recency_weight * decay(now - event_time))` would bias
toward recent memories without hard-filtering old ones.

This directly targets the second-worst category.

**6. Multi-query retrieval**

`suggest_new_ideas` (25.8%) is terrible because these questions ask the
system to synthesize — "suggest a creative outlet" needs to retrieve the
user's known interests, past suggestions, and preference patterns. A
single embedding query can't capture all of that.

Approach: the adapter (or a server-side feature) issues 2-3 retrieval
passes with different query formulations:
- Original query (what the user asked)
- Extracted entity/preference query ("music production interests of Kanoa")
- Inverse query ("what has [user] tried and didn't enjoy")

Merge results, deduplicate, feed to the LLM.

**7. Full-text search (tsvector/BM25)**

Add a `tsvector` column and GIN index on memory content. For queries
containing specific names, topics, or keywords, BM25 can find exact
matches that dense embeddings miss. Merge with cosine via RRF.

Given that Python chitta's RRF didn't improve over dense-only, this is
lower priority unless combined with other changes.

### Tier 3: higher effort (architecture additions)

**8. Cross-encoder reranking**

After initial ANN retrieval (k=50 or more), run a cross-encoder on the
top candidates to re-score query-document pairs. Cross-encoders are much
more accurate than bi-encoders for relevance but too slow for first-stage
retrieval.

BGE-reranker-v2-m3 pairs naturally with BGE-M3 embeddings. Could run via
ONNX in the same session pool infrastructure.

**9. Entity extraction and graph edges**

PersonaMem is fundamentally about *people* and their *preferences*. A
graph layer that extracts entities (person, interest, preference) and
links them with typed edges would let retrieval walk the graph:
"what does Kanoa like?" -> all edges from Kanoa node with type=preference.

This is the biggest architectural change but directly models what
PersonaMem tests.

**10. Profile-scoped preference summaries**

Periodically (or on ingestion), summarize a user's preferences into a
compact profile document. When a query mentions a user, always include
their preference summary alongside ANN results. This gives the LLM a
stable base of knowledge even when retrieval misses specific memories.

## Suggested test order

1. Run the chunk-size sweep (quick, adapter-only)
2. Try k=20/30 (adapter config change)
3. Add recency boost (small code change, targets 57.6% category)
4. Multi-query retrieval (adapter change, targets 25.8% category)
5. Cross-encoder reranking (moderate code, general accuracy lift)
6. Graph/entity extraction (large, but high ceiling)
