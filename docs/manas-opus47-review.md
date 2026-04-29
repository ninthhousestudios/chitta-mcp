# manas — opus 4.7 architectural review

Date: 2026-04-26
Reviewer: Claude Opus 4.7
Scope: `docs/manas-architecture.md`, `docs/plans/smriti-sketch.md`, `../mcpjungle` (read-through of code + README)

## summary

The architecture is good in its bones. The reframe of chitta as one subsystem (not the whole), the contracts-not-coupling discipline, and the minimal-default / opt-in-rich boot sequence are real ideas. The biggest risks are not in the design as drawn — they are in things deferred to "open questions" or named only loosely: privacy at smriti's root, freshness across subsystems, runtime portability, mental-model retirement, partial-failure semantics, multi-agent state.

On gateway: **use mcpjungle.** MPL-2.0 is clean, the feature set covers ~80% of the multi-tenancy/permission/observability concerns for free, and the gaps it has (resource templates, subscriptions, group-scoping for resources) are workable around with smriti design choices. Writing our own gateway is reinventing without the win.

On smriti: the two-tier idea, content-addressed identity, and audit-as-first-killer-feature are solid. The sketch's privacy default (index everything under `~` then ignore) is the most pressing thing to flip. Several "open questions" (mtime short-circuit, vector storage, BM25 backend) aren't open — they're requirements that shape the schema.

The doc framings — "operating system," Vedantic naming — are loose metaphors. Internally fine; if pitched externally, they'll attract pushback. Stated reuse, treated lightly.

---

## architecture doc — review

### what works

- **Reframe of chitta as one subsystem.** Conflating memory with the whole was a cognitive trap. Naming the trap broke it.
- **Contracts as English prose with explicit "what it does NOT store."** "Chitta never answers where is file Y right now" is the doc's strongest sentence — it eliminates a class of future scope creep.
- **Minimal default boot, opt-in rich boot.** The blind-session use case justifies the asymmetry; this is design, not a punt.
- **Handoff (one active file, overwritten) vs session summaries (append-only history).** Right split. Most systems get this wrong.
- **Content-addressed over path-addressed** as a principle. Pays off years later.
- **MCP as the universal interface** with explicit "if better than MCP emerges, migrate together." Honest about a commitment without dogma.

### what doesn't (or worries me)

1. **"Operating system" is doing a lot of metaphorical work.** A real OS has scheduling, memory protection, IPC primitives, syscalls. Manas has MCP servers, a markdown file the LLM reads, and skills that are structured prompts. That's a *stack* or a *runtime*, not an OS. Internal use is fine; external pitch will draw pushback.

2. **"Subsystems don't call each other; the agent orchestrates all interactions."** This makes the LLM the kernel. Every cross-subsystem coordination spends tokens and waits on inference. The genuine OS move is to allow narrowly-scoped sidebands (e.g., smriti telling chitta "hash X moved to path Y" so memory references self-heal). Banning sidebands for purity has a real cost paid in tokens forever.

3. **Skills described as "system services" but they're inert markdown.** No daemon, no lifecycle, no state. They are cron-triggerable structured prompts. Calling them services overstates what's there.

4. **No multi-agent / concurrency story.** Two CC sessions both proactively storing observations is fine (idempotency keys cover writes). Two sessions both writing `docs/handoff.md` is silent corruption. The architecture is implicitly single-tenant single-session and doesn't say so.

5. **No permission / authorization model.** Everything sees everything. Fine for solo; breaks at the moment a subagent or teammate enters. Worth at least naming as out-of-scope-for-v0.

6. **Partial-failure semantics absent.** `/done` writes session summary then handoff. If summary lands and handoff fails, half-shutdown with no recovery rule. Need to at least name what happens.

7. **CLAUDE.md is gospel with no test harness.** Every boot reads it; if a rule is wrong, every session inherits the bug silently. There is no story for regression-testing agent behavior changes from a CLAUDE.md edit.

8. **Sanskrit mapping is loose.** *smriti* in actual Vedanta is *remembered tradition / scripture* — long-term cultural memory, not filesystem perception. *chitta* is closer to "stuff of consciousness" than "memory store." Manas-as-coordinator is the cleanest fit. If anyone Vedantically literate evaluates this, the names read as decorative. Acknowledge as loose metaphor or tighten the mapping.

### things you may not know you don't know

- **Index drift.** Qartez, smriti, chitta embeddings are derived. Between rebuilds they lie. No subsystem currently emits a freshness signal; the agent has no way to weight stale results against fresh ones.
- **Privacy at smriti's root.** `~` includes `.ssh`, `.gnupg`, `.aws`, browser auth tokens in `.config/<browser>/`, `.env*` files in every project, password-manager dumps. A two-tier indexed/cataloged split doesn't solve "should this be readable by an LLM at all." Default-deny with explicit roots is the safe stance.
- **Mental-model retirement.** `/reflect` will write models. Models go wrong. "Tag, don't delete" preserves them, but trust is binary — once a model is wrong, every hit on it pollutes downstream reasoning. There is no `superseded_by`, `retired_at`, or `contradicts` mechanism designed yet.
- **Cross-cutting historical queries.** "What happened in week X?" requires chitta + smriti + git + transcripts. Each subsystem owning its history is clean per-subsystem and painful for the human asking holistic questions.
- **Token cost.** No subsystem reports what it costs to query. A rich boot could silently eat 30k tokens. Worth instrumenting before scale.
- **Runtime lock-in.** Skills are CC-specific markdown despite being called "model-agnostic." Gemini-cli running `/done` would not know to call `mcp__chittars__store_memory` unless the skill format is genuinely runner-neutral. The model-agnosticism is aspirational, not implemented.

---

## mcpjungle — use it

### license

**MPL-2.0.** File-level copyleft. Manas can use mcpjungle as a dependency without infecting itself; only modifications to mcpjungle's own files would need to be shared back. This is the deciding factor — far cleaner than qartez's dual-license, which already creates a commercial tripwire for manas.

### what it gives you

- One MCP endpoint that fronts many MCP servers — exactly the "manas as a single endpoint" framing.
- **Streamable HTTP and stdio** transports for upstream servers; gateway itself is HTTP.
- **Hot-reload**: register/deregister servers at runtime, no restart.
- **Tool Groups** — subset endpoints per client. Solves the too-many-tools-degrades-clients problem AND gives a permission primitive without writing one.
- **Prompts**: registered automatically when servers register.
- **Resources**: full proxy. URI rewriting (`mcpj://res/<server>/<base64(orig)>`), live read forwarded to upstream, annotations + meta preserved, per-resource and per-server enable/disable, both text and blob contents handled. Confirmed by code-read of `internal/service/mcp/resource.go`.
- **Enterprise mode**: users, MCP clients, ACL, OTEL metrics. Already there when it's needed.
- **Active project**: Discord, recent docs migration, April-2026 diagrams.

### gaps (smriti-relevant)

1. **No resource templates.** `ListResources` is called on registration, `ListResourceTemplates` is not. Upstream servers exposing parameterized URIs (`smriti://file/{hash}`) have those templates dropped. Workaround: smriti exposes a fixed root + tool for parameterized lookup. Better than fighting it.
2. **No resource subscriptions.** MCP supports `resources/subscribe` ("notify on change"); mcpjungle does not wire it. Workaround: smriti emits change notifications via a manas-cli sideband, not MCP-native.
3. **Tool Groups are tools-only.** Resources (and likely prompts) are not group-scoped. Per-skill / per-context permission scoping for resources is not available; tools are.
4. **OAuth not yet supported** — only static bearer tokens.

### what to keep ourselves

- **manas-cli** (god-cli, lifecycle): `manas warm`, `manas done`, `manas reflect`, `manas health`. Ops surface for the human, not MCP.
- **The smriti↔chitta sideband daemon.** Cross-subsystem coordination that doesn't belong in the LLM's token path.
- **CLAUDE.md fragment templating.** Each subsystem owns a fragment; manas-cli composes the project-level CLAUDE.md at install time. Don't try to serve CLAUDE.md via MCP — it's a CC concept, not a tool concept.

### shape

```
manas-cli (ours; god-cli, lifecycle, sideband daemon)
    │
    ├── manas warm | done | reflect | health         ops surface
    │
    └── mcpjungle (theirs; gateway, tool groups, ACL, OTEL)
            │
            ├── chitta-rs   (ours)
            ├── qartez      (external)
            └── smriti      (ours, not yet built)
```

---

## smriti sketch — review

### what's good

- **Two tiers + `[catalog]` section in `.smritiignore`.** A real idea. The `[catalog]` extension over gitignore is the kind of small, sharp innovation that makes a tool feel inevitable.
- **Backup audit as first killer feature.** Concrete, immediate, hard to argue with. Ships value before the system is "complete."
- **Content-addressed identity and the move-detection algorithm.** Correct. Fuzzy filename fallback for move+update is the right pragmatic call.
- **SQLite over Postgres.** Right tool. Single user, single writer, `sqlite3` for human inspection.
- **BGE-M3 reuse from chitta as shared tooling, not subsystem coupling.** Clean framing.

### concerns (in priority order)

1. **Privacy default is wrong.** "Everything not matching any rule is tier 1 by default" at `~` is dangerous. Under `~`: `.ssh/`, `.gnupg/`, `.aws/credentials`, `.config/<browser>/` (auth cookies, session tokens), `.mozilla/`, password-manager dumps, `.env*` in every project. Many are tier-1 readable text. Worse: BGE-M3 embeddings of those files persist to SQLite — short embedded strings (API keys, tokens) can sometimes be partially recovered from embeddings. **Flip to default-deny with explicit roots allowlist.** Or ship with hardened `.smritiignore` defaults that fail-closed on every known secret-bearing pattern. Add an embedding gate independent of indexing tier — files matching a "no-embed" glob are hashed and path-tracked but not semantically extracted.

2. **Frontmatter stripping changes identity semantics.** "Metadata-only changes don't create new identities" silently conflates "edited frontmatter" (status, version, tags) with "no edit." Hash whole content; let `minor_change` event type handle frontmatter-only diffs separately. Or strip only a small explicit list of known-noisy fields.

3. **Scan performance — mtime short-circuit is not optional.** "Re-walk only dirs with changed mtime" is in the open-questions list, but without it, `~` with 100k+ tier-1 files takes minutes per scan. Indexes go stale, system loses trust. Hash only when `(mtime, size)` disagree with the last snapshot.

4. **Vector storage at scale.** `documents.embedding BLOB` with BGE-M3 = 4 KB per row × millions = multi-GB and naive scan + cosine is unusable. **Pick `sqlite-vec` (or `sqlite-vss`) up front.** For BM25, **use SQLite FTS5**, not tantivy — it's built in, BM25-capable, no extra index files to manage. Aligns with the "single inspectable DB file" story.

5. **Freshness signal is missing from every read tool.** `smriti_health` returns `last_scan`. `smriti_find` / `_map` / `_outline` do not. The agent gets paths confidently when they may be hours stale. Add `as_of` and `is_stale` to every read envelope. Same fix applies system-wide (chitta should do this too).

6. **`smriti_read` is missing.** Current toolset assumes the agent reads files via built-in filesystem tools after smriti points at them. That bypasses smriti's exclusion model entirely — built-in Read can grab `.ssh/id_rsa` regardless of what smriti says is in scope. **Add `smriti_read(path | content_hash, range?)` as the privacy gate for file content.** Combined with mcpjungle's lack of resource templates, a tool-based read is also more pragmatic than parameterized resources.

7. **`/done` integration breaks if scans are slow.** On-demand scan of `~` is potentially minutes; /done can't block on it. The cleanest answer: smriti is a **long-lived background daemon**, not stdio-per-CC. mcpjungle proxies to it. Scan is async; tools read the latest snapshot. Stdio-per-CC works for a project-scoped scanner; not for `~`.

8. **Subscription / change-notification path.** Goes through manas-cli sideband (mcpjungle doesn't proxy MCP subscriptions). Already implied by the architecture priorities; smriti just confirms the need.

9. **Symlinks.** Open question, but the answer is opinionated: don't follow by default; record link target as data. Following symlinks under `~` can escape into `/etc`, `/var` via user-created links.

10. **Smaller things:**
    - **Hardlinks**: schema handles via multi-row paths; add a flag — modifying one modifies all, that's different from copy.
    - **`regenerable` flag is unverified.** Smriti trusts the catalog rule. Audit output should say "marked regenerable; not verified" rather than implying confidence.
    - **`smriti_find` overloaded with `content_hash` is a get, not a find.** Split into `smriti_get` or document the overload explicitly.
    - **Multi-machine**: `~/Documents` synced via Syncthing across two laptops = two divergent indexes. Out of scope, but name it explicitly.
    - **Audit by extension is a weak proxy.** `.md` is notes vs. generated docs. Topic-based audit is the real win once embeddings are in. Frame extension as a stopgap.

---

## prioritized recommendations

Tiers reflect *order of concern*, not necessarily order of work. Tier 0 items shape data formats and APIs — cheap now, expensive once code lands.

### tier 0 — bake into design before any code

1. **Smriti default-deny with explicit roots allowlist** + hardened `.smritiignore` shipped defaults + embedding gate independent of indexing tier.
2. **Freshness envelope (`as_of`, `is_stale`) on every read tool, system-wide** — chitta, qartez (where we own it), smriti.
3. **Hashing strategy for smriti — don't strip frontmatter wholesale.** Hash whole content; `minor_change` event for frontmatter-only diffs.
4. **`smriti_read` as the privacy gate for file content.** Tool-based, not parameterized resource (mcpjungle gap).
5. **Pick mcpjungle now.** Several Tier 1 / 2 items depend on this. Delaying means designing twice.
6. **Drop the "OS" framing if it's not load-bearing.** Pure doc edit, but anchors readers and invites pushback.

### tier 1 — depends on Tier 0 / mcpjungle pick

7. **Sideband cross-subsystem coordination, scoped narrowly.** With mcpjungle as the gateway, decide: which sidebands are allowed (smriti→chitta hash-move sync, scan-completion events) and where they live (manas-cli daemon). Otherwise the no-sideband rule pays a token tax forever.
8. **Permission model = lean on mcpjungle Tool Groups.** Per-skill groups, per-context groups (blind code-review session has no chitta access). Cheap, already there.
9. **Single-writer story for `docs/handoff.md`.** Advisory file lock, fail loud on contention. Decide now or face silent overwrites later.
10. **Smriti as long-lived daemon, not stdio-per-CC.** Resolves /done blocking, enables async scans, unblocks inotify later.
11. **Smriti scanner — mtime+size short-circuit from day one.** Not an optimization, a requirement.
12. **Smriti storage — sqlite-vec + FTS5 picked from day one.** Schema-shaping decision.

### tier 2 — depends on `/reflect` existing

13. **Mental-model retirement protocol.** Design before `/reflect` ships its first model. `superseded_by`, `retired_at`, `contradicts`. Without it, the first wrong model pollutes every future boot.
14. **Runtime portability — pick a side.** Either commit to CC-only and remove "model-agnostic" from the architecture doc, or factor skills into a runner-neutral spec. Don't leave it ambiguous.
15. **inotify-based watching for smriti.** Implicit v0.2 in the sketch; promote to "next thing after first ship."

### tier 3 — defer until pain

16. **Token cost instrumentation.** Wire mcpjungle's OTEL when something feels expensive. Premature now.
17. **Multi-agent / multi-user.** Enterprise mode handles it when needed.
18. **Partial-failure semantics for `/done`.** Make it idempotent and log what wrote. Full crash-only design is overkill.
19. **Cross-cutting historical queries.** Need a UX before a design.
20. **Multi-machine smriti.** Name as out-of-scope; revisit if it becomes a real workflow.

### skip

- **CLAUDE.md served as MCP resource by manas.** Couples manas to CC's loader. Keep CLAUDE.md as plain files; let manas-cli template it from subsystem-provided fragments at install time.
- **Writing your own gateway.** MPL-2.0 + the feature set kills the case.
- **Tantivy for BM25.** FTS5 is built in.

---

## open follow-ups

These are real open questions, not deferrals:

- **Resource templates in mcpjungle.** PR upstream if smriti ever needs parameterized resources, or live with the tool-based workaround.
- **Smriti incremental scan beyond mtime short-circuit** — when filesystem watching is added, what's the on-disk state if the watcher dies mid-event?
- **Embedding model versioning across chitta + smriti.** If BGE-M3 is upgraded, indexes go stale silently. Need a `model_version` column on every embedded row.
- **`document_ref` memory type in chitta.** Architecture doc flags this; resolve before smriti ships (probably remove from valid types).
- **Mental-model "trust" signal** — separate from retirement. How does the agent weight a model that's been hit on for months without correction vs. one that was just written?

---

## net take

The bones are good. The risks live in the open-questions and loose-framing categories more than in the design as drawn. The single most leveraged change is **flipping smriti's default from index-everything-then-ignore to allowlist-roots-then-extend** — that one decision shapes the privacy story, the scan performance story, and the user's first experience.

The mcpjungle decision is the unlock for several other priorities. Make it.
