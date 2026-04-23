# chitta-rs master plan

**Started 2026-04-22.** Living document. Deliberately minimal — direction comes from measurement, not speculation.

## North star

Build the best self-hosted persistent memory and learning system for the
confluence of human knowledge and AI capability.

Chitta is a **cognitive confluence** — the place where human knowledge and
agent capability meet. The agent is the primary interface consumer; the
human is the primary knowledge owner. Neither is bolted onto the other.
Both are first-class, and the combination produces something neither
could alone. Crucially, the human can disconnect — close the laptop,
walk away, and chitta waits. It's a place you come to, not something
grafted onto you.

- **Memory** — faithful, auditable recall. Verbatim storage, bi-temporal audit trails, no lossy enrichment at write time unless it earns its place.
- **Learning** — the corpus gets smarter with use. Strengthening, decay, forgetting, higher-order structure.
- **Agent-native** — tool contracts, error messages, response shapes, and latency budgets are designed for agent use. Token economy is a first-class metric. If a cold-start agent can't use the tool safely, the tool is broken.
- **Human-native** — the data belongs to the human. Knowledge graphs, views into the corpus, and the learning trajectory serve the human directly, not just through the agent. Self-hosted means the human owns and controls everything.

## Current state

chitta-rs v0.0.2 — Rust rewrite of chitta-python. Core MCP tools working: store, get, search, update, delete, list, health check. Hybrid search (semantic + full-text via RRF). Profile isolation, idempotency, bi-temporal timestamps. Running in production as Josh's daily memory server.

## Process

Benchmarks first, direction second. We don't pick features or roadmap items until we have numbers showing where chitta actually stands and where the gaps are.

1. **Stand up benchmarks** — internal regression suite and external quality (Agent Memory Benchmark). Capture baselines on the current v0.0.2 corpus.
2. **Read the numbers** — identify where retrieval quality, latency, token economy, or agent-native ergonomics fall short.
3. **Pick the next work** — let the benchmark results drive priorities, not a speculative feature list.

This cycle repeats. Ship, measure, decide.

## What we know matters (but haven't sequenced yet)

These are the broad capability areas from the Python chitta research. Their priority order is TBD pending benchmarks:

- **Retrieval quality** — hybrid ranking tuning, reranking, temporal/access-pattern weighting
- **Graph substrate** — entity graph, PageRank, co-occurrence, implicit-feedback edges
- **Learning layer** — spaced-repetition decay (FSRS), strength signals from usage patterns
- **Guardrails** — modification guards on high-value memories, blast-radius tooling
- **Token economy** — outline/summary retrieval modes, budget-capped responses, context assembly
- **Agent-native surface** — error-message quality, cold-start onboardability, response-shape consistency

## What we're not doing

- Predictive recall — wrong shape for stateless MCP
- Alternative vector backends — pgvector-committed
- Multi-agent concurrency — single-agent through v1.0, but architectural invariants (profile isolation, stateless tools, connection pooling) are maintained so it can land later
- LLM at write time — no model dependency on the write path unless benchmarks prove it necessary
