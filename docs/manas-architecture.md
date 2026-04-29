# manas — os architecture

Status: draft
Date: 2026-04-26

Manas (मनस् — "mind"). In Vedantic philosophy, manas is the coordinating faculty between perception and understanding. The naming family: **manas** (the OS) coordinates **chitta** (memory/consciousness) and **smriti** (remembering/filesystem perception), with the LLM as **buddhi** (intellect).

## what this is

Manas is an operating system for collaborative cognition — a set of subsystems with defined contracts, composed by an LLM runtime into something that has memory, perception, and continuity across sessions.

Chitta is the memory subsystem. Smriti is the filesystem perception subsystem. Neither is the whole system. This doc describes the whole system.

## why "operating system"

An MCP server answers tool calls. An operating system defines:
- What subsystems exist and what each one is responsible for
- How they interact (contracts, not coupling)
- What happens at startup, during a session, and at shutdown
- How new components join without breaking existing ones

The agent (LLM) is buddhi — the intellect that does the thinking. Everything else exists to give it perception, memory, and continuity — the cognitive infrastructure that lets one session's work survive into the next.

---

## subsystems

### memory — chitta-rs

**Contract:** store and retrieve *understanding*.

Stores: observations, decisions, mental models, session summaries, general memories.

Does NOT store: file locations, document registries, current filesystem state. Chitta answers "what do we know about X?" — never "where is file Y right now?"

Interface: MCP tools — `store_memory`, `get_memory`, `search_memories`, `update_memory`, `delete_memory`, `list_recent_memories`, `health_check`.

Governed by: `docs/principles.md`. The 11 principles apply to this subsystem specifically.

### perception: code — qartez

**Contract:** answer "what exists in the code, where, and how does it connect?"

Pre-computed index of symbols, imports, call edges, complexity, git co-change. Derived from current filesystem state — no temporal history (that's chitta's job). Rebuilds on demand.

Interface: MCP tools — `qartez_map`, `qartez_find`, `qartez_grep`, `qartez_read`, `qartez_outline`, `qartez_impact`, `qartez_deps`, etc.

Status: external tool, in use, not owned. Dual-licensed — free for current use, commercial license required at scale revenue.

### perception: documents — smriti (planned)

**Contract:** answer "what files exist, what are they about, where are they now — and where were they before?"

Content-addressed filesystem indexer rooted at `~`. Not per-project — indexes everything under the home directory. A file's identity is a hash of its content, not its path. Moves and renames are tracked, not broken.

Two tiers: **indexed** (semantically understood, hashed, lifecycle-tracked) and **cataloged** (existence + size only, for build artifacts and caches). This distinction makes the backup problem tractable — tier 1 is what you'd lose, tier 2 is what you can regenerate.

Owns its own temporal history. This is not stateless perception like qartez — files have lifecycles (created, moved, updated, deleted) and smriti tracks them. The "bespoke git" for your filesystem.

Interface: MCP tools (to be designed). See `docs/plans/smriti-sketch.md`.

Design constraints:
- Rust. Consistent with chitta-rs. Small binary, fast.
- Separate project/repo from chitta-rs. Own tool, own release cycle.
- Content-addressed with history. Snapshots of document state over time.
- Semantic indexing. Topics, structure, summaries — for agent-native queries.
- Self-hosted, human-inspectable. Aligned with principle 11.

### kernel config — CLAUDE.md

**Contract:** define behavioral rules, boot sequence, and subsystem usage instructions for the agent.

This is the most unusual subsystem because it's a text file interpreted by the LLM at runtime. It is not code — it's configuration-as-prose that the agent reads, understands, and follows. Like a shell rc file, but for a reasoning system.

Where:
- `~/CLAUDE.md` — global instructions (cross-project)
- `<project>/CLAUDE.md` — project-specific instructions

What goes here:
- Boot sequence (health checks, context loading)
- Behavioral rules (proactive observations, coding conventions)
- Subsystem pointers (how to use chitta, qartez, smriti)
- Operating constraints (what NOT to do)

What does NOT go here: project knowledge, decisions, document summaries. Those belong in chitta or in docs.

### system services — skills

Skills are operations the agent can perform, triggered explicitly by the user or on a schedule. They use the subsystems but are not part of any one subsystem.

| Skill | Role | Status |
|---|---|---|
| `/done` | Session shutdown — store observations, generate summary, write handoff | Implemented |
| `/reflect` | Maintenance — consolidate observations into mental models | Implemented |

Skills are model-agnostic. Whatever LLM runs the skill does the reasoning; the skill just defines the workflow (read these things, synthesize, store results).

### scheduler — routines

**Contract:** run system services on a schedule or one-shot at a future time.

Implementation: Claude Code routines via `/schedule`. Not yet wired to any recurring skill.

Intended use: `/reflect` runs on a schedule (daily or weekly), ensuring observations are consolidated even when Josh doesn't manually trigger it.

### IPC — handoffs

**Contract:** one active message between sessions. Forward-looking only.

Where: `docs/handoff.md` (overwritten each session, not appended).

A handoff says what the *next* session needs. Historical context lives in chitta session summaries. This separation prevents stale handoffs accumulating in search results.

### runtime — Claude Code

The shell. Manages the agent lifecycle, tool routing, permissions, hooks, and the context window. Not something we build — something we build on.

The runtime is what turns a collection of MCP servers and text files into a working system. Without it, these are just parts.

---

## session lifecycle

### boot

Two modes. CLAUDE.md loads automatically before the first message — the agent can't conditionally prevent that. So the boot behavior itself must be conditional.

**Minimal boot (default):**
1. **Health check subsystems.** Chitta first (memory is essential). Doc indexer and qartez when available. If chitta is down, tell the user immediately — the system is degraded.

That's it. The default session is lightweight. No memory loading, no handoff reading. This supports blind sessions (code reviews, fresh-eyes work) where prior context would bias the agent.

**Rich boot (opt-in):**

Triggered by the user — either explicitly ("check the handoff", "what were we working on?") or via a `/warm` skill. Adds:

2. **Read handoff.** `docs/handoff.md` has what the last session left.
3. **Search for context.** Query chitta for memories relevant to the current work.
4. *(Future)* Load active mental models. Surface contradictions or stale models.

The principle: the agent is always *capable* of continuity, but doesn't *impose* it. The user decides when prior context matters.

### run

The agent works on user tasks. During the session:

- **Observations stored proactively.** When decisions are made, preferences revealed, constraints discovered, or approaches fail. No announcement, no permission — just a `store_memory` call alongside the normal response. (Governed by CLAUDE.md behavioral rules.)
- **Perception layers queried as needed.** Qartez for code, smriti for documents, raw filesystem tools for everything else.
- **Chitta queried for context.** Relevant memories, prior decisions, mental models.

### shutdown (/done)

1. Review session for missed observations — store them.
2. Generate session summary → chitta (`memory_type: session_summary`).
3. Write `docs/handoff.md` — forward-looking notes for next session.
4. *(Future)* Signal smriti to rebuild if docs changed this session.

### maintenance (/reflect)

Runs on a schedule or manually. This is where raw material becomes knowledge.

1. Scan `.sessions/` transcripts since last reflect — extract missed observations.
2. Pull un-consolidated observations from chitta. Group by topic.
3. Synthesize into mental models. New topic → new model. Existing topic → new version that supersedes the old.
4. Mark observations as consolidated (tag, don't delete — principle 2).
5. *(Future)* Verify smriti index integrity. Surface contradictions between mental models.
6. Store a reflect receipt in chitta.

---

## component interaction rules

1. **Subsystems don't call each other.** Chitta doesn't call qartez. The smriti doesn't call chitta. The agent orchestrates all interactions. This keeps subsystems independently deployable and testable.

2. **Each subsystem owns its domain's history.** Chitta owns the history of understanding (decisions, observations, mental models). The smriti owns the history of documents (creation, moves, updates, deletions). Qartez is stateless — git already handles code history. No subsystem stores another subsystem's history. Don't put document paths in chitta; don't put decisions in the smriti.

3. **CLAUDE.md is the integration point.** The agent learns how to use each subsystem from CLAUDE.md. Adding a new subsystem means adding instructions to CLAUDE.md, not modifying existing subsystems.

4. **Degrade gracefully.** If the smriti is down, the agent can still use `find`/`grep` for documents. If qartez is down, the agent falls back to built-in file tools. If chitta is down, the agent works without memory (but warns the user). No single subsystem failure should be fatal.

5. **MCP is the universal interface.** Every subsystem that the agent interacts with programmatically exposes MCP tools. This is not a philosophical commitment to MCP specifically — it's a commitment to a uniform tool protocol. If something better than MCP emerges, the subsystems migrate together.

---

## design principles for new components

These extend (not override) the chitta-rs principles in `docs/principles.md`.

1. **Single responsibility.** Each subsystem does one thing. Don't make chitta a file indexer. Don't make the smriti store memories.

2. **Content-addressed over path-addressed.** Identity should survive moves and renames. File paths are metadata, not identity.

3. **Agent-native interface.** Design for LLM consumption: token-efficient responses, semantic queries, structured envelopes. Not human CLI output reformatted for agents.

4. **No implicit state.** Every query includes its scope (profile, path, etc.). No "current directory" or "active session" maintained server-side. The agent manages all state.

5. **Self-hosted and inspectable.** All data local. All indexes rebuildable. The human can understand what the system knows without going through an agent. Principle 11 applies system-wide.

6. **Freshness envelope on every read.** Every read tool across subsystems returns `as_of` and `is_stale`. Callers reason about staleness; the server is honest about its age. See `manas/docs/freshness-envelopes.md` for the cross-subsystem rule and per-subsystem semantics.

---

## what exists today

| Subsystem | Status | Component |
|---|---|---|
| Memory | v0.0.3 live | chitta-rs — 7 MCP tools, 5 memory types, RRF hybrid retrieval, type-weighted scoring |
| Perception: code | External, in use | qartez — 38 MCP tools, pre-computed code index |
| Perception: filesystem | Not started | Smriti — content-addressed indexer rooted at ~, two-tier (indexed + cataloged) |
| Kernel config | Active | CLAUDE.md — observation behavior, boot health check, subsystem pointers |
| /done | Implemented | Session shutdown skill |
| /reflect | Implemented (v1) | Between-session consolidation — observation grouping + mental model synthesis. No transcript scanning yet. |
| Scheduler | Available | Claude Code routines — not yet wired to recurring tasks |
| Handoffs | Active | docs/handoff.md — single overwritten file |

## what changes from the collaborative cognition plan

`docs/plans/collaborative-cognition.md` remains the implementation plan for memory types, observations, /done, and /reflect. This OS architecture doc adds:

- **Reframing:** chitta is one subsystem, not the whole system.
- **document_ref removed:** filesystem perception moved out of chitta and into smriti. The `document_ref` memory type has been removed from chitta's valid types.
- **Boot sequence:** defined as an OS concern, not a chitta feature.
- **Component contracts:** explicit boundaries on what each subsystem does and doesn't do.
- **Design principles:** for building new components that fit the system.

---

## resolved

- **Doc indexer language:** Rust. Consistent with chitta-rs. Small binary, fast.
- **Doc indexer location:** Separate repo/project. Own tool, own release cycle.
- **Boot sequence richness:** Opt-in via user request or `/warm` skill. Default boot is minimal (health check only). CLAUDE.md always loads but rich context loading is not automatic.

## open questions

- Should mental models (from /reflect) feed back into the rich boot automatically?
- What's the right trigger for doc index rebuilds — filesystem watch, session-end signal, or periodic?
- ~~What happens to the `document_ref` memory type in chitta?~~ Resolved: removed from valid types. No data existed.
- How does the smriti handle non-markdown files (PDFs, images, config files)?
