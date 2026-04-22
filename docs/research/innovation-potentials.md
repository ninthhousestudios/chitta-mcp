# chitta-rs innovation potentials

**2026-04-22.** Catalog of ideas from external research and the Python chitta roadmap, evaluated against chitta-rs's current state and north star. Nothing here is scheduled — this is the menu, not the order. Benchmarks decide priority.

**Sources:**
- Python chitta innovation roadmap (`chitta-python/docs/research/innovation-roadmap.md`) — four research tracks: entity graph, Engram steal list, Qartez steal list, Graphiti/Zep paper
- ra-h_os (`../ra-h_os/`) — local-first knowledge graph with an intent-classified retrieval pipeline
- OpenBrain / OB1 (`../OB1/`) — Nate B. Jones's open-protocol agent memory (PostgreSQL/Supabase, pgvector, Deno MCP). Three graph layers: manual ob-graph, auto entity-extraction, typed reasoning edges. Entity wiki synthesis. Cloud-first, LLM-at-write-time architecture.
- Karpathy wiki concept (via OB1 video analysis) — write-time LLM synthesis into browsable wiki pages. Hybrid proposal: structured DB as source of truth, compiled wiki as derived view.
- chitta-rs current state: hybrid RRF search (semantic + full-text), BGE-M3 1024d ONNX, pgvector HNSW, cosine similarity. Profile isolation, bi-temporal, idempotency. No graph, no LLM on write path.

---

## Proposed roadmap

Dependencies flow downward. Benchmarks gate everything — they decide priority order within each phase. Phases are sequential in concept but items within a phase can parallelize. This is a starting structure, not a commitment.

### Phase 0: Measurement (current)

Stand up benchmark harness (Agent Memory Benchmark, internal regression suite). Capture baselines on the v0.0.2 corpus. Identify where retrieval quality, latency, and token economy actually fall short.

### Phase 1: Retrieval quality

**Depends on:** Phase 0 baselines showing where pure hybrid search fails.

- Enable BGE-M3 sparse signal (already computed, currently ignored in `src/embedding.rs`)
- Three-way RRF merge (dense + sparse + FTS) — benchmark the matrix: dense-only → +sparse → +FTS → +query-variants
- Intent-gated retrieval (if benchmarks show different query types need different ranking)
- Strong-match short-circuit (if benchmarks show token waste from over-retrieval)
- Re-ranking (if precision@k metrics warrant the latency cost)

This phase is mostly tuning what already exists. Low architectural risk.

### Phase 2: Graph substrate

**Depends on:** Phase 1 retrieval foundation (need a baseline to measure whether graph-aware search actually improves things).

- Entity extraction — NER/regex, canonicalization, schema migration. The big structural addition.
- Async enrichment queue — decouple entity extraction from the write path (see below). Store verbatim immediately, queue background enrichment.
- Entity co-occurrence — sliding window counter, PMI-lite weighting.
- Typed link vocabulary — lock the edge-type enum before any edge-writing ships: `Cite`, `Contradicts`, `Refines`, `Supersedes`, `Mentions`, `HasFact`, `SimilarWeak`.
- Temporal validity on edges — `valid_from`, `valid_until`, `decay_weight` (see below). Relationships expire, not just memories.
- PageRank — memory + entity level, background recomputation. Unlocks guardrails.
- Graph-aware retrieval — follow edges from initial hits, expand neighbor context.

This is the heaviest phase. Most ideas downstream depend on it.

### Phase 3: Learning layer

**Depends on:** Phase 2 graph edges (needed for FSRS grade synthesis, implicit feedback, access-pattern signals).

- FSRS-6 spaced-repetition decay — the headline "corpus gets smarter" feature
- Implicit-feedback edge promotion — store-despite-warning → `SimilarWeak` edge
- Access-pattern boosting — frequently retrieved memories rank higher

### Phase 4: Guardrails and diagnostics

**Depends on:** Phase 2 PageRank (importance rankings needed for modification guards).

- Modification guard (PreToolUse hook, filesystem ack, TTL)
- Blast radius / memory_impact (reverse BFS over graph)
- Memory health diagnostics (read-only corpus health tool)
- Hotspot composite score (PageRank × contradictions × staleness)

### Phase 5: Compiled synthesis

**Depends on:** Phase 2 mature graph + Phase 3 learning signals. This is the furthest-out phase.

- Community detection + cluster labels (Louvain/Leiden topic partitioning)
- Derived wiki artifacts — periodic LLM synthesis from graph (see below). Not write-path, not source of truth.
- Context assembly modes (`fast|balanced|deep|decision`)
- Contradiction surfacing — audit tool that identifies tension between memories

### Continuous (any phase)

These don't gate or depend on the phase sequence:

- Error-message quality audit
- Workflow prompt templates (slash-command markdown files)
- Token economy improvements (outline retrieval is mostly shipped; budget caps exist)
- SimHash pre-embed dedup (when write volume warrants)

### Open questions for the roadmap

- **Phase 2 is too big.** Entity extraction alone was multi-phase in Python chitta. It probably needs to be broken into sub-phases once benchmarks reveal which graph capabilities matter most.
- **Where does atomic fact decomposition land?** Currently in "higher-order learning" and philosophically tense with verbatim-storage. The async enrichment queue pattern could make it viable without blocking the write path.
- **Is Phase 5 worth building, or is it a nice-to-have?** The compiled wiki idea is compelling in theory (OB1's approach) but chitta's primary user is an agent, not a human browsing Obsidian. The value proposition shifts if we get a good context-assembly tool instead.

---

## Retrieval quality

chitta-rs v0.0.2 ships hybrid RRF (dense semantic + full-text). The ideas below add further dimensions or tune the existing pipeline.

### Hybrid search (semantic + full-text + sparse)

**What:** Add term-matching retrieval alongside dense vector ANN. Merge results with Reciprocal Rank Fusion (RRF, k=60). Three signals available:

- **Dense** (current) — cosine similarity on BGE-M3 1024d embeddings. Catches paraphrases, misses exact terms.
- **Sparse** (available but unused) — BGE-M3 already produces `sparse_weights` output that chitta-rs currently ignores (`src/embedding.rs` line 33). Learned token-level importance weights — smarter than BM25 at deciding which terms matter, but opaque and not manipulable.
- **FTS** (not implemented) — BM25 via Postgres `tsvector`. Exact keyword matching, transparent, and critically the only signal that supports query-manipulation tricks (singularization, bigrams, hyphen collapsing, prefix matching — the ra-h_os patterns).

**The design question:** Sparse and FTS partially overlap — both are term-matching — but they're not identical. Sparse weights are learned (BGE-M3 knows "Rust" in a programming context matters more than "the"); BM25 weights are statistical (IDF from your corpus). Sparse can surface matches where a related token fires but the exact term doesn't appear. FTS can't do that, but FTS is the only one you can manipulate at query time.

Python chitta ran all three (dense + sparse + FTS). The open question is whether each earns its marginal contribution. The benchmark test matrix should be:

1. Dense only (current chitta-rs baseline)
2. Dense + sparse (enable BGE-M3's ignored sparse output, RRF merge)
3. Dense + FTS (add tsvector, RRF merge)
4. Dense + sparse + FTS (Python chitta's approach, three-way RRF)
5. Dense + FTS with query variants (ra-h_os style manipulation)

If dense + sparse closes 90% of the gap FTS would fill, maintaining a tsvector column + triggers + variant generation may not be worth the complexity. If there's a class of queries where only FTS-with-variants succeeds (exact identifiers, hyphenated terms, partial matches), that's the evidence for keeping all three.

**Why it matters:** This is the most obvious gap in chitta-rs retrieval. Directly improves recall on the kinds of queries agents actually make ("what did we decide about the ONNX embedder?").

**Mission alignment:** Core. Foundational infrastructure that every other retrieval improvement builds on.

**When to consider:** First priority. Benchmarks will almost certainly surface keyword-miss failures. The sparse signal is essentially free (BGE-M3 already computes it) — enable it first, then evaluate whether FTS adds enough to justify the infrastructure.

**Complexity:** Sparse: low (parse existing model output, store, RRF merge). FTS: moderate (migration, tsvector column + GIN index + trigger, query path). Both: moderate-high but well-understood.

### Intent-gated retrieval

**What:** Classify query intent before hitting the database. Route "find my note about X" differently from "explore topic X." Drop noise queries entirely. No model call — regex/heuristic classification.

**Source:** ra-h_os. They classify into: direct lookup, personal recall, focused/scoped retrieval, and noise. Each gets a different scoring function.

**Why it matters:** Not all queries want the same ranking. A recall query should favor exact matches and recency. An exploratory query should favor diversity and coverage. chitta currently treats them identically.

**Mission alignment:** High. Agent-native means understanding that agents issue different kinds of queries in different contexts. A cold-start agent exploring the corpus is not the same as an agent looking up a specific decision.

**When to consider:** After hybrid search exists and benchmarks show where ranking falls short. Intent classification is a tuning layer — it needs a retrieval foundation to tune.

**Complexity:** Low to moderate. The classification itself is cheap (regex + token counting). The value comes from routing to different scoring/ranking paths, which requires those paths to exist first.

### Query variant generation

**What:** Generate multiple FTS query variants from a single input — bigrams, singularized terms, stopword-stripped forms, hyphen-collapsed forms. Fire each as a separate full-text search, dedupe results.

**Source:** ra-h_os generates up to 6 variants per query.

**Why it matters:** Full-text search is brittle to exact phrasing. "learning strategies" won't match a memory about "learning strategy." Variant generation is a cheap way to improve FTS recall without touching embeddings.

**Mission alignment:** Moderate. Useful once full-text search exists. Pure implementation detail — agents don't see it.

**When to consider:** When hybrid search is in place and FTS recall metrics are available.

**Complexity:** Low. String manipulation, no model dependency.

### Strong-match short-circuit

**What:** When the top result scores above a high-confidence threshold, skip further expansion (graph neighbors, additional retrieval passes). Return fewer, higher-confidence results.

**Source:** ra-h_os. Score ≥1800 skips graph neighbor expansion entirely.

**Why it matters:** Agents operate in bounded context windows. Returning 10 results when the first one is clearly the answer wastes tokens. Short-circuiting respects the agent's token budget.

**Mission alignment:** High. Directly agent-native — token economy is a first-class metric.

**When to consider:** When chitta has multiple retrieval stages (hybrid search, graph expansion, re-ranking) where short-circuiting actually saves work. With the current single-ANN-pass pipeline, there's nothing to short-circuit.

**Complexity:** Low. A threshold check between pipeline stages.

### Re-ranking

**What:** After initial retrieval (ANN + FTS), apply a second-pass scorer. Options range from lightweight (BM25 re-score, cross-encoder) to heavyweight (ColBERT late interaction, LLM re-ranking).

**Source:** Python chitta research directions mentioned ColBERT-style reranking as an evaluation target.

**Why it matters:** Initial retrieval optimizes for recall (don't miss relevant results). Re-ranking optimizes for precision (put the best results first). Two-stage pipelines consistently outperform single-stage in IR benchmarks.

**Mission alignment:** Moderate. Improves quality but adds latency. The latency budget matters — agent-native means p95 targets.

**When to consider:** After hybrid search and benchmarks. If precision@5 is already good, re-ranking adds latency for marginal gain.

**Complexity:** Moderate to high depending on approach. Cross-encoder reranking needs a model; BM25 re-scoring is just math.

---

## Graph substrate

chitta-rs has no graph structure today. These ideas add entity-level and memory-level graph capabilities.

### Entity extraction and graph

**What:** Extract named entities from memories at store time. Build a memory↔entity bipartite graph. Enable entity-aware search ("find memories about Josh" even if "Josh" isn't in the query terms).

**Source:** Python chitta v0.10 — three phases: extraction, entity-aware search, canonicalization (merging aliases like `josh`/`Josh Dorfman`/`@josh`).

**Why it matters:** Entity graphs are the foundation for PageRank, co-occurrence, blast radius, guardrails, and higher-order learning. Without entities, memories are isolated documents with no structural relationships.

**Mission alignment:** Core for the "learning" pillar. The corpus can't get smarter with use if there's no structure to learn over.

**When to consider:** After retrieval quality benchmarks establish a baseline. Entity extraction is a significant architectural addition — it should be driven by evidence that structural relationships improve retrieval, not by intuition.

**Complexity:** High. Entity extraction (NER or regex), canonicalization, schema migrations, entity-aware search integration. This was a multi-phase effort in Python chitta.

### PageRank (memory + entity)

**What:** Run PageRank on two granularities: memories linked by edges, and entities linked by co-occurrence. Store as columns (`memories.pagerank`, `entities.pagerank`). Background recomputation, not per-write.

**Source:** Python chitta innovation roadmap (merged from Engram #9 + Qartez #2).

**Why it matters:** PageRank surfaces structural importance — which memories are load-bearing in the graph. Enables guardrails (don't delete high-PR memories), hotspot detection, and importance-weighted retrieval.

**Mission alignment:** High for "learning" — importance is a learned signal. Depends on entity graph existing first.

**When to consider:** After entity graph lands and has enough edges to make PageRank meaningful.

**Complexity:** Moderate. The algorithm is commodity; the operational question is when/how to recompute.

### Entity co-occurrence

**What:** Track which entities appear together in memories. `entity_cooccurrences(a, b, count, last_seen_at)` with PMI-lite weighting.

**Source:** Python chitta innovation roadmap (Engram #10). Directly completes the entity graph with entity↔entity edges.

**Why it matters:** Co-occurrence captures relationships the memory text doesn't state explicitly. "Josh" and "chitta" co-occurring frequently is a structural signal even if no memory says "Josh works on chitta."

**Mission alignment:** Moderate. Enriches the graph substrate. Value depends on how retrieval uses it.

**When to consider:** Alongside or shortly after entity graph.

**Complexity:** Low. Sliding window counter, one table.

### Typed link vocabulary

**What:** Add `edge_type` enum to memory links: `Cite`, `Contradicts`, `Refines`, `Mentions`, `HasFact`, `SimilarWeak`.

**Source:** Python chitta innovation roadmap (Engram #11).

**Why it matters:** Undifferentiated edges lose information. Knowing that memory A *contradicts* memory B is different from knowing they're *related*. Typed edges enable smarter traversal and richer blast-radius analysis.

**Mission alignment:** Moderate. Schema decision that should be made early if we're going to have edges at all — retrofitting types onto untyped edges is painful.

**When to consider:** Before any edge-writing feature ships. Lock the vocabulary first.

**Complexity:** Low. Schema decision + enum column.

### Temporal validity on edges

**What:** Add `valid_from TIMESTAMPTZ`, `valid_until TIMESTAMPTZ`, and `decay_weight FLOAT (0.0–1.0)` to edges. A relationship can be "currently true" (`valid_until IS NULL`), "expired" (`valid_until < now()`), or "decaying" (weight decreasing over time). Graph traversal respects validity windows and weights.

**Source:** OB1 `schemas/typed-reasoning-edges/schema.sql`. Their `thought_edges` table tracks when a reasoning relationship (supports, contradicts, supersedes) became true, when it stopped, and how confident/fresh it is.

**Why it matters:** Relationships change. "Josh uses Python" was true, then "Josh switched to Rust" superseded it. Without temporal validity, the old edge persists at full weight and pollutes graph traversal. This is the temporal axis that chitta's bi-temporal model doesn't yet cover — chitta has bi-temporal *memories* but undifferentiated *edges*.

**Mission alignment:** High for "learning" — a corpus that tracks evolving relationships is smarter than one with static edges. Natural extension of typed link vocabulary. Makes the `Supersedes` and `Contradicts` edge types actionable rather than just labels.

**When to consider:** Design alongside typed link vocabulary — these columns should ship in the same schema migration. Retrofitting temporal validity onto existing untyped edges would be painful.

**Complexity:** Low. Three columns + a validity predicate in graph traversal queries.

### Async enrichment queue

**What:** A work queue table that decouples entity extraction from the write path. A trigger on `INSERT`/`UPDATE` of memory content queues a row; a background worker processes the queue asynchronously (extract entities, build edges, update co-occurrences).

Schema sketch (adapted from OB1's `entity_extraction_queue`):
```sql
enrichment_queue (
  id UUID PRIMARY KEY,
  memory_id UUID REFERENCES memories(id),
  status TEXT DEFAULT 'pending',  -- pending/processing/complete/failed/skipped
  attempt_count INT DEFAULT 0,
  queued_at TIMESTAMPTZ DEFAULT now(),
  started_at TIMESTAMPTZ,
  completed_at TIMESTAMPTZ,
  worker_version TEXT
)
```

**Source:** OB1 `schemas/entity-extraction/schema.sql`. Their queue is populated by a Postgres trigger on every thought INSERT/UPDATE, processed by a separate worker that calls an LLM for entity extraction.

**Why it matters:** This is the pattern that reconciles chitta's "no LLM on write path" principle with the desire for rich entity extraction. The write path stays fast and model-free (store verbatim, embed, return). Enrichment happens asynchronously — could be seconds later, could be a scheduled batch. The memory is immediately searchable; entities arrive eventually.

**Mission alignment:** Core. Solves the central tension in chitta's architecture: the north star says "no model dependency on the write path unless benchmarks prove it necessary" but the graph substrate needs entity extraction. The queue makes enrichment a read-path optimization rather than a write-path dependency.

**When to consider:** Build alongside entity extraction — this is the *mechanism* for entity extraction, not a separate feature. Design the queue before the extractor so the extraction pipeline is async from day one.

**Complexity:** Low-moderate. The queue itself is simple (one table, one trigger, status FSM). The worker is where the complexity lives, but the queue decouples the design decision of *when* to enrich from *how* to enrich.

---

## Learning layer

Ideas that make the corpus get smarter over time — the "learning" pillar of the north star.

### FSRS-6 spaced-repetition decay

**What:** Replace simple TTL/reinforce with FSRS-6 (Free Spaced Repetition Scheduler). Tracks retrievability and stability per memory. Four-grade feedback synthesized from existing signals: `contradict → Again`, `recall → Good`, `reinforce → Easy`, no-event-in-window → implicit `Hard`.

**Source:** Python chitta innovation roadmap (Engram #1).

**Why it matters:** Scientifically grounded forgetting curve. Memories that get recalled strengthen; memories that get contradicted weaken; memories nobody touches gradually fade. This is the core "learning" thesis — the corpus adapts to usage patterns.

**Mission alignment:** Core for "learning" pillar. This is the headline feature that distinguishes a learning system from a storage system.

**When to consider:** After benchmarks establish retrieval quality baseline. FSRS changes ranking in ways that are hard to evaluate without a benchmark harness. The grade-synthesis step (mapping agent actions to FSRS grades) needs careful validation.

**Complexity:** High. The FSRS algorithm is published and portable, but grade synthesis from implicit signals is the risky design problem. Needs A/B benchmarking to validate.

### Implicit-feedback edge promotion

**What:** When an agent stores a memory despite a conflict warning (similarity 0.75–0.85 to an existing memory), auto-create a `SimilarWeak` edge. The store-despite-warning gesture is implicit feedback that the memories are meaningfully related.

**Source:** Python chitta innovation roadmap. Block-0 dogfood evidence: a memory had a 0.755-similarity neighbor that got `conflict_warning` but `links_created=0`.

**Why it matters:** Cheap learning signal from existing behavior. No new API, no model call — just interpreting an action the agent already takes.

**Mission alignment:** High. Learning from usage patterns without explicit user/agent cooperation is exactly the north star.

**When to consider:** After edge infrastructure exists. Low risk, low cost, high signal.

**Complexity:** Low. One conditional in the store path.

---

## Guardrails and diagnostics

Ideas that protect the corpus and surface its health.

### Modification guard

**What:** Block destructive operations (delete, contradict, consolidate) on high-PageRank memories until the agent has called `find_related` recently. External binary in a PreToolUse hook, filesystem-based ack with ~10-minute TTL.

**Source:** Python chitta innovation roadmap (Qartez #1). Described as "the single most novel UX idea" across all research tracks.

**Why it matters:** Agents make mistakes. A cold-start agent with no context about corpus importance can nuke load-bearing memories. The guard forces a "look before you leap" workflow.

**Mission alignment:** High for agent-native. This is designing for the failure mode of the primary user (an AI agent). But it requires PageRank to know which memories are high-value.

**When to consider:** After PageRank exists and we have enough corpus to make importance rankings meaningful.

**Complexity:** Moderate. Separate binary, filesystem state, TTL logic. The interesting design question is where it lives (client hook vs. server-side check).

### Blast radius / memory_impact

**What:** Reverse BFS over the memory + entity graph. "Deleting this memory orphans entities X, Y, Z; invalidates 14 reinforcements." Depth-capped at 3 hops.

**Source:** Python chitta innovation roadmap (Qartez #3). Ships with the modification guard as one UX.

**Why it matters:** Makes the consequences of destructive operations visible before they happen. Information tool, not a blocker.

**Mission alignment:** Moderate. Useful for the guard workflow. Standalone value is limited without the graph substrate.

**When to consider:** Alongside the modification guard.

**Complexity:** Moderate. One recursive CTE.

### Memory health diagnostics

**What:** Read-only tool returning corpus health: counts, coverage, duplicate pairs above threshold, orphan entities, entity/edge coverage gaps.

**Source:** Python chitta innovation roadmap (Engram #5).

**Why it matters:** Surfaces problems the agent (or human) should address. "N memories have zero entity edges" is actionable.

**Mission alignment:** Moderate. Diagnostic, not retrieval. But agent-native means agents need to self-diagnose corpus problems.

**When to consider:** Low cost, useful at any point. More valuable once there are more dimensions to diagnose (entities, edges, PageRank).

**Complexity:** Low. Read-only queries, one tool.

### Hotspot composite score

**What:** Rank memories by `PageRank × (1 + contradict_count) × log(1 + recall_count) × staleness`. Surfaces memories needing curation — the inverse of the guard ("touch these" vs. "don't touch").

**Source:** Python chitta innovation roadmap (Qartez #4).

**Why it matters:** Proactive corpus maintenance. Instead of waiting for retrieval failures, surface the memories most likely to cause problems.

**Mission alignment:** Moderate. Diagnostic tooling. Value scales with corpus size.

**When to consider:** After PageRank and enough usage signals to make the composite meaningful.

**Complexity:** Low. One scoring query.

---

## Token economy and agent surface

Ideas that make chitta cheaper and easier for agents to use.

### Outline-style retrieval

**What:** Two-tier API: search returns `[{id, title_or_first_line, snippet, score, tags}]`; separate batch call fetches full content. Server-side `max_tokens` budget cap on every retrieval tool.

**Source:** Python chitta innovation roadmap (Qartez #5). Python chitta shipped this.

**Why it matters:** chitta-rs already returns 200-char snippets and has `max_tokens` budget pruning — this is partially implemented. The missing piece is the explicit two-tier contract where agents choose when to fetch full content.

**Mission alignment:** Core for agent-native. Token economy is a first-class metric.

**When to consider:** chitta-rs is already close. The gap is mainly API design clarity, not implementation.

**Complexity:** Low. Mostly already there.

### Context assembly modes

**What:** `context(query, mode=fast|balanced|deep|decision, max_tokens)` returns a budgeted multi-layer context pack. Opinionated API that assembles retrieval results into a structured context for the agent.

**Source:** Python chitta innovation roadmap (Engram #6).

**Why it matters:** Agents currently have to chain multiple tool calls to build context. A single-call assembly is more token-efficient and less error-prone.

**Mission alignment:** Mixed. Convenient for agents, but opinionated. Trades flexibility for ease of use. The alternative (workflow prompt templates that chain existing tools) is less code and more adaptable.

**When to consider:** After benchmarks show whether the retrieval quality supports differentiated modes. If `fast` and `deep` return basically the same results, the modes are fake.

**Complexity:** Moderate. The assembly logic is the hard part — what goes in each mode and why.

### Error-message quality

**What:** Every error message includes: (1) what the agent did, (2) why it was rejected, (3) what to do next with a concrete example.

**Source:** Python chitta innovation roadmap. Track 3 agent-native quality.

**Why it matters:** Agents can't interpret vague errors. "Error: invalid input" is useless. "store_memory: tags must be a JSON array, got string '\"foo\"'. Use tags: [\"foo\"]" is actionable.

**Mission alignment:** Core for agent-native. If a cold-start agent can't recover from errors, the tool is broken.

**When to consider:** Any time. Doesn't depend on other features. Audit existing error paths against this standard.

**Complexity:** Low. Per-error-path review and rewrite.

### Workflow prompt templates

**What:** Markdown slash-command templates (`/chitta_recall`, `/chitta_profile`, `/chitta_audit`, etc.) chaining 2–4 existing tools. Zero code — just prompt files.

**Source:** Python chitta innovation roadmap (Qartez #6).

**Why it matters:** The onboarding surface for agents that aren't Claude Code. Templates codify best-practice tool chains without requiring the agent to discover them.

**Mission alignment:** High for agent-native. Zero cost to maintain, high value for cold-start agents.

**When to consider:** Once the tool surface is stable enough that templates won't immediately break.

**Complexity:** Very low. Markdown files.

---

## Deduplication

### SimHash pre-embed dedup

**What:** Compute SimHash (locality-sensitive hash) on memory content at store time. Skip embedding generation for near-duplicates. One column (`simhash BIGINT` + index), Hamming distance threshold.

**Source:** Python chitta innovation roadmap (Engram #2). Python chitta shipped this.

**Why it matters:** Saves embedding compute for near-duplicate stores. Deterministic, no model dependency. Especially valuable for agents that retry stores or import overlapping content.

**Mission alignment:** Moderate. Cost reduction on the write path. chitta-rs already has idempotency keys for exact dedup; SimHash catches near-duplicates.

**When to consider:** When write volume is high enough that embedding cost matters, or when benchmarks show near-duplicate pollution in retrieval results.

**Complexity:** Low. ~50 lines, one migration.

---

## Higher-order learning

### Atomic fact decomposition

**What:** Extract atomic claims from memories at write time, store alongside the verbatim source, link via `HasFact` typed edge. Enables fact-level contradiction detection and retrieval.

**Source:** Python chitta innovation roadmap (Engram #3). Previously on the skip list due to verbatim-storage thesis.

**Why it matters:** "Josh prefers Rust and uses Arch Linux" contains two independent facts. Fact-level storage enables precise contradiction ("Josh switched to NixOS" contradicts the second fact but not the first) and finer-grained retrieval.

**Mission alignment:** Tension. This is write-time LLM enrichment, which the north star says to avoid "unless it earns its place." Benchmarks would need to show significant retrieval improvement to justify the model dependency and latency on the write path.

**When to consider:** Only if benchmarks show that memory-level retrieval consistently fails on fact-scope queries. This is a Tier 3+ item — high value if the evidence supports it, but it violates a default principle.

**Complexity:** High. NLP/LLM extraction pipeline, schema changes, edge management, write-path latency impact.

### Community detection + cluster labels

**What:** Partition the memory graph into topic clusters using Louvain/Leiden community detection. Label each cluster.

**Source:** Python chitta innovation roadmap (Engram #8 / Qartez).

**Why it matters:** Enables a "topics" view of the corpus. Useful for corpus exploration and diversity in retrieval results.

**Mission alignment:** Low-moderate. More useful for human browsing than agent retrieval. Agent-native value is mainly in diversity — ensuring search results span multiple topic clusters.

**When to consider:** Only if users ask for a topics browser, or benchmarks show retrieval diversity problems.

**Complexity:** Moderate. Algorithm is commodity; labeling heuristic is the design problem.

---

## Compiled synthesis

Ideas that generate derived artifacts from the graph — human-browsable or agent-consumable views that are regenerated from source-of-truth data, never edited directly.

### Derived wiki artifacts

**What:** A periodic compilation agent reads the entity graph, community clusters, and typed edges, then generates per-entity or per-topic markdown pages via LLM synthesis. Pages include: summary, key facts, timeline, relationships, open questions, contradictions. Output modes: markdown files on disk (browsable in Obsidian), or stored back as `dossier`-typed memories with embeddings (searchable by agents).

The critical architectural constraint: **the wiki is a materialized view, not a source of truth.** The database + graph is always authoritative. If the wiki has an error, fix the source data and regenerate. The wiki is never edited directly by humans or agents.

**Source:** OB1 `recipes/entity-wiki/`. Inspired by Karpathy's wiki concept but inverted: Karpathy's wiki IS the knowledge store; OB1's wiki is a derived view over a structured database. Nate Jones's hybrid proposal (2026 video): "Keep OpenBrain as your permanent store. A wiki layer acts as a compiled view on demand."

**Why it matters:** Addresses two gaps: (1) agents currently reconstruct understanding from raw memories on every query — a compiled synthesis pre-builds cross-references and connections. (2) human browsability — chitta is deliberately headless, but a generated wiki provides a browsable artifact without compromising the headless architecture.

The Karpathy insight underlying this: most AI knowledge tools spend compute to *rederive* understanding on every query. A compiled wiki *compiles* understanding once and keeps it current. The key difference from Karpathy's approach is that compilation happens from structured data, not raw files — so you can filter by date, weight by confidence, exclude stale items, and get richer synthesis than raw-file ingest allows.

**Mission alignment:** Mixed. The primary user of chitta is an AI agent, not a human in a browser — so the human-browsability argument is secondary. The stronger argument is agent-facing: a compiled context per topic could replace expensive multi-memory retrieval with a single pre-built artifact. But this is speculative without benchmarks showing that query-time synthesis is actually a bottleneck.

**When to consider:** Phase 5 at earliest. Requires mature entity graph, typed edges, and ideally community detection. The compilation agent needs rich structure to synthesize from — without it, you're just running LLM summarization over flat memory lists, which isn't much better than query-time synthesis.

**Complexity:** Moderate. The LLM synthesis is the easy part. The hard design problems: (1) when to regenerate (schedule? on-demand? triggered by graph changes?), (2) staleness detection (how does the system know a wiki page is outdated?), (3) scope boundaries (what defines a "topic" worth compiling?), (4) whether the output is agent-searchable (stored as memories with embeddings) or purely for human consumption.

### Contradiction surfacing

**What:** An audit tool that scans the memory graph for tensions: memories connected by `Contradicts` edges, entity-level conflicts (two memories assert different things about the same entity), temporal supersessions that haven't been flagged. Produces a report of unresolved contradictions ranked by importance (PageRank of involved memories).

**Source:** OB1 video. Nate Jones flags this as a key weakness of both approaches — wikis smooth contradictions into coherent narratives (hiding them), databases store contradictions silently in adjacent rows (ignoring them). Neither surfaces them proactively.

**Why it matters:** Contradictions are often the most valuable signal in a knowledge base. "Engineering says 12 weeks, sales promised 8" — resolving that into a synthesis loses the strategic signal. A contradiction-aware system preserves tension and surfaces it when relevant.

**Mission alignment:** High for "learning" — a corpus that detects its own inconsistencies is smarter than one that ignores them. Depends on typed edges (`Contradicts`, `Supersedes`) and entity graph to be meaningful.

**When to consider:** After typed edges and entity graph ship. Could start simple (report all `Contradicts` edges) and grow sophisticated (entity-level conflict detection across memories that don't share explicit edges).

**Complexity:** Low-moderate. The simple version is one query over typed edges. The sophisticated version (cross-memory entity-level conflict detection) requires entity extraction + co-occurrence + some reasoning.

---

## Permanently skipped

These were evaluated across multiple research sources and rejected:

- **Predictive recall** — wrong shape for stateless MCP
- **AST shape clone detection** — SimHash covers natural-language dedup
- **Alternative vector backends (LanceDB, etc.)** — pgvector-committed
- **FSRS as end-user grading API** — Anki-UI shape, not agent shape; grade synthesis stays internal
- **Engram coordination services (axon/brain/broca)** — platform scope creep
- **Graphiti bi-temporal edges with per-write LLM extraction** — the full Graphiti pattern (LLM extraction blocking the write path) conflicts with verbatim-storage thesis. However, two sub-ideas are adopted independently: temporal validity on edges (`valid_from/valid_until/decay_weight`, see Graph substrate) and async enrichment queue (decouples extraction from write path). The Graphiti *architecture* is skipped; specific *mechanisms* are taken.
- **Convention-based dedup** (ra-h_os "search before creating") — fragile vs. structural dedup (idempotency keys, SimHash)
- **Multi-language code parsing, watch mode, toolchain runner** — not memory-shaped

---

## What benchmarks need to answer

This catalog is deliberately unsequenced. The benchmarks we're about to build should answer:

1. **Where does pure semantic search fail?** — keyword misses, exact-term queries, identifier lookup. If these are common, hybrid search (RRF) is the first priority.
2. **Does retrieval quality degrade with corpus size?** — if recall drops as memories accumulate, we need importance-weighted ranking (PageRank, FSRS decay).
3. **Are agents wasting tokens on retrieval results?** — if search returns 10 results when 1–2 are relevant, intent gating and strong-match short-circuit matter.
4. **Do cold-start agents succeed with current tool descriptions?** — if not, error-message quality and workflow templates are the fix, not more features.
5. **What's the latency budget?** — determines whether re-ranking, graph expansion, and context assembly modes are viable.

The answers determine which items from this catalog become the next work.
