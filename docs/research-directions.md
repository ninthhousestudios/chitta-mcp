# chitta-rs research directions

Chitta is not just a memory store — it's a research platform for understanding how persistent memory changes the relationship between humans, agents, and learning. This document captures the longer-term research directions that inform architectural decisions.

---

## 1. Retrieval quality

The foundational research question: given a corpus of personal memories, how do you return the *right* ones?

**Strategies to evaluate:**
- Pure semantic (cosine similarity) — current v0.0.1 baseline
- BM25 full-text — keyword precision for exact terms, names, identifiers
- Hybrid RRF (Reciprocal Rank Fusion) — Python chitta's current approach
- Late-interaction reranking (ColBERT-style) — token-level similarity after initial retrieval
- Temporal weighting — recency bias, decay functions
- Access-pattern boosting — frequently retrieved memories rank higher
- Tag-aware retrieval — hard filters vs. soft boosts
- Graph-aware retrieval — follow edges from initial hits

**Evaluation approach:**
- Query logs from real usage (Phase 0 of v0.0.2) provide the replay corpus
- PersonaMem benchmark for preference recall (baseline: 60% with Python chitta)
- Custom eval sets built from actual "did the agent find what it needed?" sessions
- A/B comparison: replay same query set against two strategies, measure MRR/precision@k

**Open questions:**
- What's the right embedding model? BGE-M3 is the current choice (1024d, multilingual). Is a smaller model with reranking better than a bigger model alone?
- Does the nature of personal memory (short, heterogeneous, context-dependent) favor different strategies than document retrieval?
- How much does temporal context matter? A decision from yesterday vs. six months ago.

---

## 2. Learning-from-decisions

The insight: engineering decisions are domain education. A memory system that recognizes this would be genuinely new.

**What "recognition" means:**
- Inferring transferable principles from specific decisions. "You chose a session pool for the embedder" → "you understand concurrent resource management."
- Cross-project pattern matching. The session pool in chitta-rs and a connection pool in Drishti are the same pattern — surface this without being asked.
- Implicit preference detection. 15 decisions that all favor simplicity over flexibility → "this person values minimalism" without it ever being stated.
- Skill emergence. Capabilities derived from what you've built, not self-assessed.

**What it could produce:**
- A capability profile that accumulates as a byproduct of real work
- Reflective queries: "what am I learning?" not just "what do I know?"
- Transfer suggestions: "you solved something similar here" when the domains differ but the principle matches
- Contradiction detection: "this decision conflicts with your usual pattern — intentional?"

**Research questions:**
- What's the representation? A tag? A derived embedding? A graph cluster?
- How do you distinguish "I did this once" from "I understand this deeply" (repetition, coherence, building-on-prior)?
- Can this be done at retrieval time (query-side) or does it require a periodic synthesis pass?
- Is this a chitta feature or a layer on top of chitta?

---

## 3. Assessment and proof-of-work

The ecosystem context: resumes are dead, vibecoders can produce code but not understanding. Platforms like Talentboard and goship.tech are building "prove you can do it" systems. They use assessment (perform for an evaluator). An alternative: continuous evidence accumulation from real work.

**The thesis:**
A trail of architectural decisions, made coherently over time, is unfakeable proof of capability. You can't vibecode your way through 6 months of decisions that build on each other.

**What an export/proof layer would need:**
- Privacy controls: which memories/decisions are shareable, which aren't
- Verification: how does a third party trust the trail is real? (git commit hashes, timestamps, cryptographic signing?)
- Legibility: raw decision memories aren't readable by strangers. Need a synthesis layer that produces a readable capability narrative from raw evidence.
- Granularity: "I know Rust" is useless. "Here are 12 concurrency decisions I made correctly, with context" is compelling.

**Relationship to chitta:**
- Chitta already stores the raw material (decisions with timestamps, tags, context)
- The "learning-from-decisions" layer (section 2) would identify the capabilities
- The export layer would package them for external consumption
- This is probably v0.1.0+ territory — needs the foundation to be solid first

---

## 4. PersonaMem and preference benchmarks

Current state: Python chitta scores 60% on PersonaMem. Competing systems score 80%+.

**Why chitta underperforms:**
- Chitta stores decisions and facts but doesn't synthesize them into preference patterns
- Preferences are implicit in the decision trail but never extracted as first-class objects
- Retrieval may find relevant memories but not recognize them as preference-evidence

**Research directions:**
- Periodic preference synthesis: scan recent decisions, extract "this person prefers X over Y" as derived memories
- Query-time aggregation: when asked about preferences, don't just retrieve — aggregate across multiple memories
- Explicit vs. implicit: is it better to extract and store preferences (brittle, can go stale) or infer them at query time (expensive, but always fresh)?
- What does 90%+ look like? Is it even desirable, or does over-fitting to stated preferences miss evolving taste?

---

## 5. Multi-agent memory sharing

Not immediate, but relevant to the architecture: when multiple agents (Claude Code sessions, Drishti, future tools) share a memory store, what are the dynamics?

- Profile isolation vs. shared knowledge
- Write conflicts: two sessions store contradictory decisions about the same thing
- Read patterns: does one session's retrieval benefit from another session's queries?
- Agent-specific vs. human-facing memories: should an agent's internal working memory (scratchpad) live in the same store as durable human knowledge?

---

## Sequencing

| Phase | Focus | Depends on |
|-------|-------|-----------|
| v0.0.2 | Instrumentation, query logging, pipeline modularity | — |
| v0.0.3 | Migration, baseline eval on real corpus | v0.0.2 query logs |
| v0.0.4+ | Hybrid retrieval experiments, PersonaMem improvements | v0.0.3 corpus |
| v0.1.0+ | Learning-from-decisions, capability profiles | Retrieval quality solved |
| Future | Assessment/proof-of-work export | Everything above |

The key constraint: you can't research retrieval quality without real data flowing through the system. v0.0.2 and v0.0.3 exist to build that foundation. The interesting research starts once the corpus is live and the query logs are accumulating.
