# ra-h_os research notes

**2026-04-22.** Analysis of [ra-h_os](../../../ra-h_os/) — a local-first personal knowledge graph. TypeScript/Next.js, SQLite + sqlite-vec, Tauri desktop wrapper, MCP server. Different goals than chitta (knowledge graph vs. agent memory), but the retrieval pipeline has ideas worth studying.

## What it does

Stores atomic "nodes" (ideas, articles, people, decisions, transcripts) in SQLite with typed edges between them. Exposes a browser UI and an MCP server. Two separate MCP implementations: one talks to the running Next.js app, one hits the SQLite file directly (standalone, works without the app).

## Retrieval pipeline — the interesting part

ra-h's retrieval is a multi-stage pipeline with intent classification. This is where it diverges from chitta's current "every query gets the same treatment" approach.

### Stage 1 — Intent gate (no DB hit)

Regex-based classification before touching the database:
- Drops noise queries ("ok", "hi", "test") entirely
- Routes "find my note about X" differently from "what's related to X"
- Routes "inside this transcript" to focused/scoped retrieval
- Only retrieves if query is ≥12 chars or ≥3 tokens

### Stage 2 — Query variant generation

Generates up to 6 FTS query variants from a single input, no model calls:
- Phrase bigrams from adjacent non-stopword tokens
- Singularized term combinations ("strategies" → "strategy")
- Topical-term-only variants (filters note-type words like "idea", "thought")
- Hyphen-collapsed variants ("all-in" → "allin")

Each variant fires a separate FTS search; results deduped by node ID.

### Stage 3 — Dual scoring functions

Two separate rankers depending on classified intent:

**Search scoring** (general lookup):
- Exact title match: +2000, title starts with query: +1200, title contains: +700
- Per-term occurrence counts weighted by field (title 40pt, description 8pt, source 3pt)
- Recency tiebreaker

**Recall scoring** ("find something I wrote"):
- Human-authored content bonus: +800
- Exact phrase match in title: +2500
- Full term coverage bonuses
- Explicitly boosts `captured_by === 'human'` nodes

### Stage 4 — Strong-match short-circuit

When a direct hit scores ≥1800, graph neighbor expansion is skipped entirely. Prevents padding results with loosely related noise when you found exactly what was asked for.

### Stage 5 — Graph neighbor expansion

If no strong match: top 3 seed nodes get their edges fetched, up to 2 neighbors per seed added with `kind: 'neighbor'`. Graph traversal surfaces contextually related nodes the user didn't name.

### Stage 6 — Chunk retrieval (conditional)

Only for focused-source or source-detail queries. Long content is pre-chunked into passages with separate embeddings. Returns up to 4 passage previews at 220 chars.

### Hybrid search (optional path)

FTS + LIKE + relaxed LIKE merged with vector results using **Reciprocal Rank Fusion** (k=60). The standard path skips vector search for speed; hybrid is opt-in.

## Ideas relevant to chitta

**Intent-gated retrieval.** chitta currently treats every `search_memories` identically. A recall query ("what did I decide about X") and an exploratory query ("anything about Rust") get the same ranking. Distinguishing these at the query level — without any model call — is cheap and would improve result quality.

**Query variant generation for full-text.** When chitta adds full-text search, generating FTS variants (bigrams, singularization, stopword stripping) from the original query would improve recall without touching embeddings. Pure string manipulation.

**Strong-match short-circuit.** If the top result is a near-perfect match, stop adding more. Reduces noise in agent context windows.

**Dual scoring.** The insight that "find my note" and "explore a topic" are different retrieval problems is correct. Different scoring functions for different intent classes would let chitta tune each independently.

## Ideas not relevant to chitta

- **Tech stack** — SQLite/sqlite-vec is a different world from Postgres/pgvector.
- **Convention-based dedup** ("search before creating") — fragile vs. chitta's idempotency keys.
- **OpenAI embedding dependency** — chitta's local ONNX embedder is the right call.
- **Skills as graph nodes** — orthogonal to memory.
- **Chunk-level retrieval** — chitta stores short memories, not documents. Revisit if that changes.
- **Tauri/Next.js UI** — chitta is headless MCP, no UI planned.

## Bottom line

The retrieval pipeline is genuinely thoughtful. The pattern of intent-classify → route → score-differently → short-circuit is something chitta should consider once benchmarks show where retrieval quality falls short.
