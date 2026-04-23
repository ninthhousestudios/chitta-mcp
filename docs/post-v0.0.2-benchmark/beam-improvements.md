# BEAM benchmark improvements

Baseline: chitta-rs v0.0.2, BEAM 100k split, **66.6%** (303/400).
Ingestion: 292s for 170 docs (1720ms/doc). Retrieval: 877ms avg.

## Failure analysis

| Category | Accuracy | Notes |
|---|---|---|
| preference_following | 95.0% (38/40) | Near-ceiling |
| summarization | 95.0% (38/40) | Near-ceiling |
| abstention | 92.5% (37/40) | Near-ceiling |
| temporal_reasoning | 92.5% (37/40) | Strong |
| event_ordering | 85.0% (34/40) | Strong |
| instruction_following | 80.0% (32/40) | Good |
| contradiction_resolution | 67.5% (27/40) | Weak |
| information_extraction | 62.5% (25/40) | Weak |
| knowledge_update | 45.0% (18/40) | **Bad** |
| multi_session_reasoning | 42.5% (17/40) | **Worst** |

97 wrong answers total. 70 are total misses (score=0), 27 are partial.

Context tokens are identical for right (9,250) and wrong (9,336) —
again a context quality problem, not quantity.

## Failure patterns

### knowledge_update: retrieving stale values (45%)

Of 22 wrong answers, **19 returned an old value** and only 3 couldn't
find anything. The system retrieves the original fact ("budget is $600")
but not the update ("budget increased to $650"). Both memories exist in
the store but cosine similarity ranks them equally since both are
semantically close to the query. Without temporal weighting, there's no
preference for the newer value.

Examples:
- "How many commits?" Gold: 165. Got: 150 (original value).
- "Test coverage?" Gold: 78%. Got: 65% (original value).
- "Sneaker budget?" Gold: $650. Got: $600 (original value).

This is the single clearest failure mode — the retrieval finds relevant
content but returns the wrong version.

### multi_session_reasoning: can't aggregate across memories (42.5%)

These questions require counting, comparing, or synthesizing information
scattered across multiple conversation sessions. Dense retrieval returns
individual memories but can't aggregate them.

Examples:
- "How many columns did I want to add?" Needs to combine requests from
  separate sessions.
- "How many features did I mention?" Requires counting across all
  conversations about the weather app.

A single embedding query cannot capture "find everything related to X and
count the distinct items." This needs either multi-query retrieval or a
structured representation (entities/graph).

### contradiction_resolution: not surfacing both sides (67.5%)

Gold answers are empty lists — the correct response is to identify a
contradiction. The LLM sees one side in the retrieved context and answers
confidently. To get this right, retrieval must surface both the assertion
and its contradiction. Since contradictory statements are semantically
similar, they should both appear in results — the issue may be that only
one version makes it through the context window, or the LLM isn't
prompted to check for contradictions.

### information_extraction: precision gaps (62.5%)

Mix of "not found in context" (retrieval miss) and imprecise answers
(right topic, wrong details). Less systematic than the other failures.

## Improvement ideas

### Tier 1: directly targets worst categories

**1. Recency-weighted scoring (targets knowledge_update: 45%)**

This is the highest-leverage single change. 19 of 22 knowledge_update
failures returned a stale value because cosine similarity doesn't
distinguish old from new.

Approach: score = `cosine_similarity * (1 + w * recency_factor)` where
`recency_factor` decays from 1.0 (just stored) to 0.0 (oldest memory).
The weight `w` controls how much recency matters — start with 0.1-0.2 to
gently bias without destroying semantic relevance.

Implementation: can be done in the SQL query using `event_time` (already
stored). No schema change needed.

**2. Multi-query retrieval (targets multi_session_reasoning: 42.5%)**

Issue multiple retrieval passes with different query formulations, merge
and deduplicate results. For aggregation questions ("how many X across
sessions"), the adapter or a server-side feature extracts the entity and
issues a broad query ("all mentions of [entity]") alongside the original
query.

This is an adapter-side change for the benchmark, or a new tool
(`search_aggregate`?) server-side for production use.

**3. Contradiction-aware context assembly (targets contradiction: 67.5%)**

When retrieved results contain memories with high cosine similarity to
each other but different content, flag the potential contradiction in the
context. This helps the LLM recognize that conflicting information exists
rather than just answering from whichever memory appears first.

### Tier 2: general quality improvements

**4. Increase k and improve context assembly**

Default k=10 may miss relevant memories. For BEAM's multi-session
questions, k=20-30 with deduplication could capture more of the scattered
information. The context assembly step should also prioritize diversity
(not just top-k similarity) to avoid returning near-duplicate memories
that waste context tokens.

**5. Cross-encoder reranking**

After retrieving k=50 candidates, re-score with a cross-encoder to push
the most relevant memories to the top. This would help information_extraction
(finding exact details) and knowledge_update (preferring the precise
answer over a semantically similar but outdated one).

**6. BGE-M3 sparse weights**

Same as PersonaMem — the sparse signal is already computed but discarded.
Worth testing but the failure patterns here (stale values, aggregation)
are not the kind that sparse retrieval fixes.

### Tier 3: structural changes

**7. Entity/fact tracking**

Extract structured facts from memories: `(entity, attribute, value,
timestamp)`. When a query asks "what is X?", look up the latest value
for that entity-attribute pair. This directly solves knowledge_update
and simplifies multi_session_reasoning.

**8. Memory versioning / supersede links**

When a new memory updates a fact that exists in an older memory, link
them with a "supersedes" edge. At retrieval time, always prefer the
latest version. This cleanly solves the stale-value problem without
needing recency heuristics.

## Comparison with PersonaMem

| | PersonaMem 32k | BEAM 100k |
|---|---|---|
| Accuracy | 64.3% | 66.6% |
| Worst category | suggest_new_ideas (25.8%) | multi_session_reasoning (42.5%) |
| Key failure mode | Can't synthesize preferences | Can't aggregate across sessions + stale values |
| Temporal issue | Preference evolution (57.6%) | Knowledge update (45%) |

Both benchmarks expose the same two gaps:
1. **No temporal signal** — cosine similarity treats old and new equally
2. **No aggregation** — single-query retrieval can't span multiple memories

Recency-weighted scoring is the highest-leverage single change across
both benchmarks. Multi-query retrieval is second.

## Suggested test order

1. Recency-weighted scoring (directly targets 45% knowledge_update)
2. Increase k to 20-30 (quick adapter config, helps multi_session)
3. Multi-query retrieval (adapter change, helps both worst categories)
4. Cross-encoder reranking (moderate code, general lift)
5. Entity/fact extraction (large, high ceiling for knowledge_update)
