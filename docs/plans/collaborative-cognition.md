# collaborative cognition architecture

Status: draft
Date: 2026-04-25

## context

Chitta today is a passive memory store — agent writes, agent reads. All cognitive labor of *what* to remember and *when* to remember it falls on the human. This is not collaborative.

This design adds three capabilities:

1. **Memory types** — first-class categorization that drives retrieval behavior
2. **Proactive observation** — agent stores observations during sessions without prompting
3. **Reflect** — async synthesis of raw material into consolidated knowledge

Architecture influenced by Hindsight (vectorize.io): retain/recall/reflect with typed memory bank. Adapted for local-first, human-owned design.

---

## memory types

New column on `memories`:

```sql
ALTER TABLE memories ADD COLUMN memory_type text NOT NULL DEFAULT 'memory';
CREATE INDEX idx_memories_type ON memories (memory_type);
CREATE INDEX idx_memories_profile_type_record
  ON memories (profile, memory_type, record_time DESC);
```

### type definitions

| Type | Purpose | Lifecycle | Example |
|---|---|---|---|
| `memory` | General-purpose. Default. Backward-compat. | Permanent | "chitta-rs v0.0.2 released 2026-04-20" |
| `observation` | Granular thing noticed during a session | Ephemeral — raw material for mental models | "Josh pushed back on write-time extraction, wants benchmarks first" |
| `decision` | Structured decision record | Permanent | "Decided: RRF with k=60. Rationale: ..." |
| `session_summary` | Recap of a session | Permanent | "Worked on RRF implementation, hit sparse leg issues" |
| `mental_model` | Consolidated understanding from observations | Long-lived, superseded by newer versions | "Josh values evidence over intuition for architecture" |
| ~~`document_ref`~~ | *(Removed — filesystem perception moved to smriti)* | — | — |

### backward compatibility

Default is `memory`. All existing rows remain valid with no migration of content. Tags continue to work alongside types — they're orthogonal. Type is structural (affects system behavior), tags are descriptive (affect query filtering).

### retrieval weighting

Types carry optional scoring weights as a post-fusion multiplier:

| Type | Default weight | Rationale |
|---|---|---|
| `mental_model` | 1.3 | Consolidated knowledge, highest signal |
| `decision` | 1.2 | Structured, intentional |
| `memory` | 1.0 | Baseline |
| `session_summary` | 1.0 | Useful context |
| ~~`document_ref`~~ | *(removed)* | Moved to smriti |
| `observation` | 0.7 | Raw material, not yet consolidated |

Configurable via `CHITTA_TYPE_WEIGHTS` env var. Integrates into the existing RRF pipeline.

---

## the three layers of reflect

### layer 1: during-session (proactive observations)

As conversation happens, the agent stores observations without being asked. This is always-on behavior driven by a CLAUDE.md instruction block, not a skill.

**Store when:**
- A decision is made (with rationale and rejected alternatives)
- Josh corrects something or pushes back (captures preferences and values)
- An approach is tried and fails (negative knowledge)
- A non-obvious constraint or requirement surfaces
- Something would be hard to reconstruct from transcript alone

**Don't store:**
- Routine code changes
- Things already captured in docs or code
- Trivial exchanges
- Restating what's obvious from the transcript

**Format:** 1-3 sentences. `memory_type: observation`. Topical tags. No announcement — just a `store_memory` call alongside the normal response.

### layer 2: session-end (/done skill)

Invoked explicitly (`/done`) or eventually via a session-end hook.

1. Review conversation for missed observations — store them
2. Generate session summary (`memory_type: session_summary`)
   - What was worked on, outcomes, decisions, open threads
   - Pointer to transcript `.sessions/<uuid>.jsonl`
3. Write `docs/handoff.md` — forward-looking notes for next session
4. *(Removed — document tracking moved to smriti)*

This replaces the manual "please save a summary" ritual.

### layer 3: between-session (/reflect skill)

Runs via scheduled routine (daily/weekly) or manual invocation. This is where synthesis happens.

1. **Transcript scan** — read `.sessions/` files since last reflect. Extract observations missed during live sessions.
2. **Observation consolidation** — pull un-consolidated observations. Group by topic. Synthesize into mental models.
3. **Mental model update** — for each topic with new observations:
   - Existing model → create new version, `metadata.supersedes: <old_id>`, tag old as `superseded`
   - No model → create new mental model
4. *(Removed — document registry moved to smriti)*
5. **Mark observations consolidated** — tag with `consolidated` (don't delete — principle 2).
6. **Store reflect receipt** — `session_summary`-type memory recording what reflect did.

**Model options for running reflect:**
- Claude via scheduled Claude Code agent — most capable, costs tokens
- Local model (ollama, etc.) — cheap, needs to be good enough at synthesis
- Cheaper hosted model — middle ground

The skill is model-agnostic. It calls chitta MCP tools and reads files. Whatever runs the skill does the LLM reasoning.

---

## document registry (moved to smriti)

Document awareness has moved out of chitta into the smriti filesystem perception subsystem. See `docs/manas-architecture.md` for the architectural rationale and `docs/plans/smriti-sketch.md` for the design.

---

## tool changes

### store_memory — new parameter

```
memory_type: Option<String>  // default "memory", validated against known types
```

### search_memories — new parameter

```
memory_types: Option<Vec<String>>  // filter by type(s), OR-match like tags
```

### list_recent_memories — new parameter

```
memory_types: Option<Vec<String>>  // filter by type(s)
```

### get_memory — no parameter changes

Returns `memory_type` in response (already returned as part of full row).

---

## principles alignment

| # | Principle | Assessment |
|---|---|---|
| 1 | Verbatim is sacred | Aligned. Mental models are new rows, never mutations. |
| 2 | Bi-temporal | Aligned. Supersession via new rows + metadata reference. |
| 3 | Write fast, enrich lazily | Aligned. `memory_type` is just a column. Reflect is explicitly async. |
| 4 | Agent-native wire contract | Aligned. Envelope unchanged. `memory_type` in results. |
| 5 | Small core, grow by evidence | Aligned. No new tools. Existing tools gain a parameter. |
| 9 | No write-time extraction | Aligned. During-session observations are agent-initiated, not extraction pipelines. |
| 11 | Human owns the data | Aligned. All types queryable, inspectable, exportable. |

---

## implementation order

Each step independently shippable and testable.

1. **Schema + tool changes** — migration 0005, add `memory_type` to store/search/list args, update MemoryRow
2. **Type-weighted retrieval** — post-fusion multiplier in retrieval.rs
3. **CLAUDE.md behavior block** — during-session observation rules
4. **/done skill** — session-end reflect
5. **/reflect skill** — between-session consolidation + mental model synthesis *(implemented — v1 without transcript scanning)*
6. ~~**Document registry**~~ — *(moved to smriti)*
7. **Scheduling** — wire /reflect to a routine

---

## open questions

- Should `memory_type` be a Postgres enum or a text column with application-level validation? Text is more flexible for iteration; enum is safer. Leaning text with validation in `validate.rs`.
- Should observations have a TTL or only get cleaned up during reflect? Leaning toward reflect-only — no silent data loss.
- What's the right granularity for mental models? Per-topic? Per-domain? This probably emerges from usage rather than being designed upfront.
- How should the session-end hook work mechanically? `user-prompt-submit` hook matching "done"/"exit"? A custom slash command? Both?
- Retrieval: should `search_memories` exclude observations by default, or include everything and let the type weights handle ranking? Leaning toward include-with-weights — exclusion hides data.
