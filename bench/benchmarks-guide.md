# AMB benchmarks guide

Which benchmarks in [agent-memory-benchmark](https://github.com/...) matter for chitta, and why.

## Evaluation modes

**Retrieval-only** (`bench/retrieval-eval.py`): measures recall@k, MRR, hit rate, gold term overlap. No LLM, zero API cost. Tests whether the right memories surface. This is the primary eval mode for chitta — retrieval quality is chitta's job; the consuming agent handles synthesis.

**RAG** (`omb run`): full pipeline with an answer LLM + judge LLM. Tests end-to-end accuracy. Useful for comparing against published baselines, but the LLM choice dominates results and doesn't reflect real usage where a capable agent (Claude, etc.) consumes retrieved memories.

## Benchmarks

### PersonaMem

- **Source**: [PersonaMem](https://huggingface.co/datasets/nicholascpark/personamem)
- **Size**: 195 docs, 589 queries (~1.4M tokens), split: 32k
- **Task type**: multiple-choice
- **Isolation**: batch (ingest all, query all)
- **Categories**: generalizing_to_new_scenarios, provide_preference_aligned_rec, recall_user_shared_facts, recalling_facts_mentioned_by_the_user, recalling_the_reasons_behind_previous_updates, suggest_new_ideas, track_full_preference_evolution
- **Estimated sweep time**: ~3 min/config

**Value for chitta**: moderate. Good for fast iteration since it runs quickly. Tests preference recall and evolution tracking. Weakness: MCQ format, single batch ingest (no conversation isolation), doesn't test temporal reasoning deeply. We've largely exhausted what this benchmark can tell us — retrieval recall is 94-97% across configs.

### BEAM

- **Source**: BEAM benchmark
- **Isolation**: per-conversation (ingest one conversation, query, reset)
- **Task type**: open-ended

**Value for chitta**: high. Per-conversation isolation is more realistic than PersonaMem's batch mode. Open-ended questions test retrieval quality in a more natural setting. Currently running RRF sweep.

### LifeBench

- **Source**: [LifeBench](https://github.com/1754955896/LifeBench) (ICLR 2026 Memory Agent Workshop)
- **Size**: 3,605 docs, 2,003 queries (~12M tokens), split: en
- **Task type**: open-ended
- **Isolation**: per-user (10 users, ~360 docs each)
- **Categories**: information-extraction (0), multi-hop (1), temporal-updating (2), nondeclarative (3), unanswerable (4)
- **Estimated sweep time**: ~30 min/config, ~3.5 hours for 7-config sweep

**Value for chitta**: highest. Simulates a full year of daily life for 10 fictional users with multi-source digital traces (SMS, calls, photos, calendar, fitness). Tests exactly the agent-with-persistent-memory scenario chitta targets. Key differentiators vs other benchmarks:
- Multi-source data (not just conversations)
- Long horizon (365 days)
- Temporal reasoning (evolving information over time)
- Unanswerable detection (should NOT retrieve confidently when answer doesn't exist)
- Nondeclarative memory (habits, preferences, emotions — not just facts)

Note: user names are Chinese despite the English QA pairs. Docs are large (mean ~13K chars, max ~38K).

### LoCoMo

- **Source**: [LoCoMo](https://github.com/snap-research/locomo) (Snap Research)
- **Size**: 10 multi-session conversations, open-ended QA
- **Task type**: open-ended
- **Isolation**: per-conversation
- **Categories**: single-hop (1), temporal (2), multi-hop (3), open-domain (4), adversarial (5)

**Value for chitta**: moderate-high. Conversation-centric, which matches chitta's primary use case. The adversarial category is unique — tests whether the system gets tricked by misleading questions. Smaller scale than LifeBench but well-established in the literature.

### LongMemEval

- **Source**: [LongMemEval](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned)
- **Size**: ~500 questions, each with its own session haystack
- **Task type**: open-ended
- **Isolation**: per-question (each question gets its own memory bank)
- **Categories**: single-session-user, single-session-assistant, multi-session, temporal-reasoning, knowledge-update, single-session-preference

**Value for chitta**: moderate. Good coverage of temporal reasoning and knowledge-update scenarios. Weakness: per-question isolation means each eval creates a tiny, fresh memory bank — doesn't test retrieval from a large persistent store. Less realistic for chitta's use case but useful for targeted temporal/knowledge-update testing.

### MemBench

- **Source**: [MemBench](https://github.com/import-myself/Membench)
- **Splits**: FirstAgentLowLevel, FirstAgentHighLevel, ThirdAgentLowLevel, ThirdAgentHighLevel
- **Task type**: multiple-choice
- **Data**: requires manual download from Google Drive

**Value for chitta**: low. Tests abstraction levels (low-level vs high-level memory) and perspective (first-person vs third-person). The abstraction dimension is interesting conceptually but tests LLM reasoning more than retrieval quality. Manual download adds friction.

### MemSim (MemDaily)

- **Source**: [MemSim](https://github.com/nuster1128/MemSim)
- **Size**: multiple trajectories with single QA each
- **Task type**: multiple-choice
- **Language**: Chinese only
- **Categories**: simple, conditional, comparative, aggregative, post_processing, noisy

**Value for chitta**: low. Chinese-language only. BGE-M3 handles Chinese fine but our test infrastructure, users, and documentation are English. The "noisy" category (robustness to distractor memories) is interesting but not worth the language barrier.

## Priority order

1. **BEAM** — in progress, RRF sweep running
2. **LifeBench** — next after BEAM. Richest benchmark for chitta's use case
3. **LoCoMo** — if a third data point is needed. Adversarial category is unique
4. **LongMemEval** — only if temporal/knowledge-update needs targeted testing
5. **MemBench** — skip
6. **MemSim** — skip
