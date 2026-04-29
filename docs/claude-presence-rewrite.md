# presence — rewrite plan

Status: plan
Date: 2026-04-26
Context: `docs/manas-architecture.md`, `docs/manas-opus47-review.md` (Tier 1 concurrency item), upstream `~/soft/claude-presence` (~1100 LOC TypeScript, MIT)

## why rewrite

The upstream `claude-presence` is well-scoped, well-documented, MIT-licensed, and small enough to fork (~1100 LOC across 9 TypeScript files). Its **design** is sound: presence registry + named resource locks + broadcast inbox, SQLite-backed, no daemon, advisory trust model.

The reasons to rewrite rather than vendor:

1. **Language consistency.** Manas's other owned subsystems (chitta-rs, smriti) are Rust. Adding a Node runtime to manas's dependency surface is a real cost: another toolchain to install, another set of supply-chain risks (npm), another set of failure modes for users. Rewriting in Rust keeps the manas binary surface uniform.
2. **Aesthetic preference.** Stated explicitly. Acknowledge it as a real reason — unmaintained code in a language you dislike is unmaintained code.
3. **Manas-specific extensions.** Cross-project / user-scope locks, integration with mcpjungle, manas-cli daemon shape, and a canonical lock vocabulary are all easier to bake in during a clean rewrite than to graft onto an upstream we don't control.
4. **Single maintainer upstream, v0.1 status.** Upstream is `garniergeorges/claude-presence`, marked "🚧 v0.1 — early development. API may change." Building a manas dependency on an unstable v0.1 single-maintainer project is risky regardless of language.

What we are **not** doing: improving the design. The upstream design is correct for the problem. The rewrite preserves it.

---

## naming

Two reasonable options, deferred to Josh:

- **`presence`** — pragmatic, descriptive, lowercase + hyphen-friendly. Crate name `presence-rs`, MCP server `presence-mcp`.
- **`sangha`** (सङ्घ — "assembly / community / those who gather") — fits the Vedantic-loose-metaphor family alongside chitta and smriti. Crate name `sangha-rs`. *Sangha* in actual usage is closer to the right meaning than smriti was — community of practitioners — so the metaphor holds up better than most.

This doc uses `presence` throughout for clarity; substitute as decided.

---

## what we keep from upstream

The whole design. Specifically:

- **SQLite, single file, WAL mode.** No daemon coordination needed; SQLite's locking handles many-readers + one-writer naturally.
- **Per-project scoping.** Sessions and locks default to scoped by project (cwd or explicit project_id).
- **Self-declared session IDs, advisory locks.** Cooperating-local-sessions trust model. Documented as such.
- **Heartbeat-based session liveness.** Default 10-minute TTL; sessions ping to stay alive.
- **TTL on locks.** Default 10 min, configurable per-claim, max 24 h. Force-release available.
- **Broadcast inbox.** Soft channel — advisory messages between overlapping sessions. Treated as advisory only; agents don't act on inbox content without user verification.
- **Tool surface.** The 9 MCP tools stay 1:1 in the rewrite (see *MCP tools* below).
- **CLI for human inspection.** `presence status`, `presence locks`, `presence clear`, `presence path`.
- **Slash commands.** `/register`, `/claim`, `/release`, `/presence`, `/broadcast`, `/inbox`. Same UX.

---

## what we change

### 1. Cross-project (user-scope) locks

Upstream is project-scoped only. Manas needs *some* user-scope locks because not every coordination need is project-bound:

- `/reflect` operates on the user's whole chitta — two sessions in different projects must not run it concurrently.
- A future `smriti-scan` triggered by /done is user-wide too (smriti's index spans all allowlisted roots, not one project).
- Periodic maintenance routines via `/schedule`.

**Schema change:** make `project` nullable, with `NULL` denoting user-scope. Or keep `project NOT NULL` and use a reserved sentinel like `__user__`. The sentinel is more explicit and avoids `WHERE project IS NULL` semantics churn — go with sentinel.

**API change:** `resource_claim` accepts a `scope: "project" | "user"` field. `"project"` is default; `"user"` rewrites the project field to `__user__` server-side. The slash command grows a `--user` flag (or a separate `/claim-user`).

### 2. Lock vocabulary, canonical

Upstream lets callers name any resource. Open vocabulary is fine for users but bad for cross-skill coordination — two skills could pick different names for the same resource and not coordinate.

**Manas defines a canonical set** in CLAUDE.md and in each skill's prose:

| Resource | Scope | Default TTL | Held by |
|---|---|---|---|
| `handoff` | project | 120 s | `/done` while writing `docs/handoff.md` |
| `session-summary` | project | 120 s | `/done` while writing chitta session summary (only if conflicts emerge) |
| `reflect:user` | user | 600 s | `/reflect` for the entire run |
| `smriti-scan` | user | 300 s | the smriti daemon while scanning, or any caller invoking on-demand scan |
| `chitta-write` | user | optional | reserved; probably never needed (idempotency keys cover writes) |

Callers can still use ad-hoc names. The canonical names are documented so skills don't drift.

### 3. Integration with mcpjungle

Upstream is stdio MCP. The rewrite stays stdio, registered with mcpjungle like chitta and smriti. Upstream's tools are flat-named (`session_register`, `resource_claim`, etc.); through mcpjungle they become `presence__session_register`. Rename internally to match the chitta-rs pattern of grouped names if desired (`presence__session_register` would already be unambiguous through mcpjungle namespacing — no internal rename needed).

### 4. Long-operation heartbeat semantics

Upstream's lock TTL is independent of heartbeat — a lock survives session crashes only as long as its TTL. For long-running operations that don't make MCP calls (e.g., a /done step that runs an embedding job), the session heartbeat must continue or session liveness lapses.

Two improvements in the rewrite:

- **Auto-extend on heartbeat.** When `session_heartbeat` is called, locks held by that session whose TTL is more than half-expired get bumped to full TTL. Avoids needing explicit lock-renew.
- **Long-op annotation.** `resource_claim` accepts `long_op: bool`. When set, TTL defaults to a longer value (30 min) and is auto-extended more aggressively on heartbeat. /reflect uses this; /done usually doesn't.

### 5. Server-side session identity hardening (lightweight)

Upstream `from_session` is fully self-declared on every call — a session can post broadcasts as another session. The README acknowledges this and points to issue #1 ("derive `from_session` from the MCP connection context").

In the rewrite: **derive session identity from the MCP connection** for the lifetime of a stdio process. The first `session_register` call binds an identity to the connection; subsequent calls on the same connection can't claim a different identity. This doesn't make presence cryptographically authenticated — a different process can still register a duplicate ID — but it removes the trivial "one tool call impersonates another session" footgun for free.

For the cooperating-local trust model this is sufficient. Keep the docs honest: "advisory, not enforced; cooperating sessions only."

### 6. Inbox: keep but de-emphasize

The broadcast inbox is upstream's smallest feature and the one with the least clear value. Keep it because:

- It's cheap (one table, two tools).
- It surfaces overlap context to the human ("session-a1b2 was working on X at 14:00") without us having to invent another channel.
- Other sessions reading the inbox is *advisory*, not coordination — so the trust-model concerns don't matter much.

But document it as: **the inbox is for humans / surfacing context; it is not a coordination protocol.** Skills should not act on inbox messages. Manas-cli `warm` may surface them at boot.

### 7. Manas integration points

Beyond the rewrite itself, the manas integration adds:

- **SessionStart hook auto-`/register`s** the session with branch + cwd + initial intent (which can be empty). Removes the manual `/register` step.
- **UserPromptSubmit hook** (already in upstream as a script) injects a one-line "other sessions / locks active" message into context. Port the shell script logic into manas's hook layer; reuse the upstream `presence status --json` shape.
- **`/done` claims `handoff`** before writing handoff.md, releases after. Surfaces conflict to user if claim fails. Encoded in the /done skill prose.
- **`/reflect` claims `reflect:user`** for the run, releases after. Skips with a polite message if held.
- **manas-cli `warm`** reads recent inbox messages and surfaces them as context.
- **manas-cli `health`** includes a "presence reachable, N other sessions, M held locks" line.

---

## architecture

### crate layout

```
presence/
├── Cargo.toml
├── src/
│   ├── main.rs           -- CLI entry: serve | status | locks | clear | path
│   ├── mcp.rs            -- MCP server (rmcp) + tool handlers
│   ├── tools/
│   │   ├── mod.rs
│   │   ├── presence.rs   -- session_register, _heartbeat, _unregister, _list
│   │   ├── locks.rs      -- resource_claim, _release, _list
│   │   └── inbox.rs      -- broadcast, read_inbox
│   ├── db.rs             -- rusqlite connection pool, migrations
│   ├── schema.sql        -- embedded via include_str!
│   ├── identity.rs       -- connection-bound session identity
│   ├── ttl.rs            -- lock TTL math, auto-extension
│   ├── cli.rs            -- human-facing CLI (status, locks, clear, path)
│   └── config.rs         -- env vars
├── migrations/
│   └── 0001_initial.sql
└── tests/
    ├── presence_test.rs
    ├── locks_test.rs
    ├── inbox_test.rs
    ├── identity_test.rs       -- connection-bound identity invariants
    └── concurrency_test.rs    -- two MCP clients claim same resource
```

Estimate: ~1500 lines of Rust including tests, vs. ~1100 LOC of TypeScript (no tests in the count). Rust is more verbose for plumbing but tighter in logic. Tests are non-negotiable.

### dependencies

- `rmcp` — MCP server framework, same as chitta-rs.
- `rusqlite` (with `bundled` feature) — SQLite. No external sqlite required.
- `tokio` — async runtime.
- `serde` / `serde_json` — tool input/output.
- `clap` — CLI surface.
- `tracing` / `tracing-subscriber` — logs.
- `gethostname` — for session metadata.
- `uuid` — session ID generation.

No `sqlite-vec` or `tantivy` needed; presence is structured-only.

### transport

stdio. One process per Claude Code instance, same as upstream. No daemon needed — SQLite handles concurrency. (Smriti is a daemon because of scan latency; presence has no equivalent long-running work.)

Through mcpjungle, presence is registered like any other stdio MCP server.

---

## schema

Largely the upstream schema with the cross-project change and a small addition. SQL, kept close to upstream so behavior is verifiable against existing test cases:

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,             -- cwd-derived project key, or '__user__' for user-scope
    branch TEXT,
    intent TEXT,
    pid INTEGER,
    hostname TEXT,
    started_at INTEGER NOT NULL,       -- unix millis
    last_heartbeat INTEGER NOT NULL,
    metadata TEXT                       -- JSON blob, free-form
);
CREATE INDEX idx_sessions_project ON sessions(project);
CREATE INDEX idx_sessions_heartbeat ON sessions(last_heartbeat);

CREATE TABLE resource_locks (
    resource TEXT NOT NULL,
    project TEXT NOT NULL,             -- '__user__' for user-scope locks
    session_id TEXT NOT NULL,
    branch TEXT,
    reason TEXT,
    long_op BOOLEAN NOT NULL DEFAULT FALSE,  -- NEW: long-operation flag
    acquired_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (project, resource),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX idx_locks_expires ON resource_locks(expires_at);
CREATE INDEX idx_locks_session ON resource_locks(session_id);

CREATE TABLE inbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    from_session TEXT NOT NULL,
    from_branch TEXT,
    message TEXT NOT NULL,
    tags TEXT,                         -- JSON array
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_inbox_project ON inbox(project, created_at DESC);

CREATE TABLE inbox_reads (
    session_id TEXT NOT NULL,
    message_id INTEGER NOT NULL,
    read_at INTEGER NOT NULL,
    PRIMARY KEY (session_id, message_id)
);
```

Constants (env-overridable):

| Constant | Default | Env |
|---|---|---|
| `SESSION_TTL_MS` | 10 min | `PRESENCE_SESSION_TTL_SEC` |
| `LOCK_DEFAULT_TTL_MS` | 10 min | `PRESENCE_LOCK_TTL_SEC` |
| `LOCK_LONG_OP_TTL_MS` | 30 min | `PRESENCE_LOCK_LONG_OP_TTL_SEC` |
| `LOCK_MAX_TTL_MS` | 24 h | `PRESENCE_LOCK_MAX_TTL_SEC` |
| `INBOX_RETENTION_MS` | 24 h | `PRESENCE_INBOX_RETENTION_SEC` |
| DB path | `~/.presence/state.db` | `PRESENCE_DB_PATH` |

---

## MCP tools

All prefixed `presence__` through mcpjungle. Tool surface 1:1 with upstream plus minor additions:

### session_register

```
Input:  { project?: string, branch?: string, intent?: string, metadata?: object }
Output: { session_id: string, others: [SessionSummary] }
```

`project` defaults to derived-from-cwd. `session_id` is generated server-side and bound to the MCP connection.

### session_heartbeat

```
Input:  {}
Output: { session_id: string, ttl_remaining_sec: int }
```

Bumps `last_heartbeat` and auto-extends locks past half-life.

### session_unregister

```
Input:  {}
Output: { ok: bool }
```

### session_list

```
Input:  { project?: string, scope?: "project" | "user" | "all" }
Output: { sessions: [Session], as_of: timestamp }
```

Default scope = `project`. `user` shows only user-scope locks; `all` shows everything visible.

### resource_claim

```
Input:  {
    resource: string,
    scope?: "project" | "user",       -- NEW
    reason?: string,
    long_op?: bool,                   -- NEW
    ttl_sec?: int                     -- override default
}
Output: {
    ok: bool,
    held_by?: { session_id, branch, since, reason },
    expires_at?: timestamp
}
```

### resource_release

```
Input:  { resource: string, scope?: "project" | "user", force?: bool }
Output: { ok: bool }
```

`force: true` releases a lock you don't hold. Used by `presence clear` and emergency recovery.

### resource_list

```
Input:  { scope?: "project" | "user" | "all" }
Output: { locks: [Lock], as_of: timestamp }
```

### broadcast

```
Input:  { message: string, tags?: [string], scope?: "project" | "user" }
Output: { id: int }
```

### read_inbox

```
Input:  { filter?: "unread" | "all", limit?: int }
Output: { messages: [InboxMessage], as_of: timestamp }
```

Marks returned messages as read for this session.

### Freshness envelope

Same convention as smriti: `as_of` (now-ish) on every read tool. Presence is real-time so `is_stale` doesn't apply, but `as_of` is useful for client-side reasoning.

---

## CLI

Human-facing, mirrors upstream:

```bash
presence status                    # active sessions on this project
presence status --project .        # explicit project filter
presence status --user             # user-scope sessions and locks
presence status --json             # machine-readable

presence locks                     # active resource locks
presence locks --user

presence clear                     # prune dead sessions and expired locks
presence clear --force-release X   # force-release a specific lock

presence path                      # print SQLite DB path
presence help
```

---

## hooks integration

Two hooks ship with manas. Both are minimal shell wrappers that call the `presence` binary; the binary does the actual work.

### SessionStart

Auto-`/register` the session. Keeps presence visible to other sessions from the moment Claude Code starts. (Upstream chose manual /register for explicitness. For manas, the trade-off is different — we want SessionStart to make the session discoverable for /reflect coordination and inbox messages from prior overlapping sessions.)

```bash
#!/usr/bin/env bash
# Auto-register; failure is non-fatal.
presence register \
    --branch "$(git -C "$PWD" branch --show-current 2>/dev/null || true)" \
    --cwd "$PWD" \
    >/dev/null 2>&1 || true
```

### UserPromptSubmit

Surface presence context as a one-liner. Port of the upstream shell script:

```bash
#!/usr/bin/env bash
status=$(presence status --project "$PWD" --json 2>/dev/null) || exit 0
# emit a one-line system message: "N other sessions, M locks active"
# only if N > 0 or M > 0
```

Both hooks degrade silently if the `presence` binary is missing. Manas's installer adds these to settings.json, merging with existing hook entries (the same merge pattern upstream documents).

### Skill integration

Skill prose in /done and /reflect (markdown), not hooks:

- **/done** — early in the skill, attempts `presence__resource_claim resource=handoff scope=project ttl_sec=120`. On success, proceeds. On conflict, surfaces the holder to the user and offers options ("wait", "broadcast a coordination message", "abort"). After writing handoff.md, releases.
- **/reflect** — claims `reflect:user`, scope=user, long_op=true. Skips with a message if held.
- **/warm** (when implemented) — reads inbox messages from prior overlapping sessions, surfaces them as context.

---

## migration story

There is no manas v0.1 yet. There is no production presence usage yet. **No migration is needed** — we ship the rewrite, configure manas to use it, never touch upstream.

If Josh has been using upstream `claude-presence` informally, the data shape (sessions, locks, inbox) is identical enough that a one-shot import script could move `~/.claude-presence/state.db` to `~/.presence/state.db`. Probably not worth writing — re-register sessions, re-claim locks, done.

---

## open questions

- **Project key derivation.** Upstream uses cwd; subprojects (worktrees, monorepos) have different cwds for the same logical project. Add an optional `~/.presence/projects.toml` mapping cwds to canonical project keys? Or just trust cwd? Trust cwd v0; revisit when monorepo pain shows up.
- **Multiple worktrees of the same repo.** A common case: two CC sessions in two worktrees of the same git repo, on different branches. Are they "the same project" (both compete for `handoff`?) or different? Probably *different* — each worktree has its own `docs/handoff.md`. cwd-based project keys handle this naturally (different cwds → different projects). Document the behavior so it's not surprising.
- **Inbox spam.** No rate limiting upstream. Probably fine for cooperating sessions; revisit if anything goes wrong.
- **Forwarding to a remote presence server.** Upstream is local-only. If multi-machine ever matters (e.g., Josh on laptop and desktop both running CC against a shared repo via Syncthing), we'd want a network-backed presence. Out of scope for the rewrite; named here so it's not forgotten.
- **Identity binding lifetime.** Connection-bound identity is per stdio process. If a session restarts and re-registers with a different generated ID, locks held by the old ID expire on TTL. Acceptable but worth documenting — locks aren't transferable across restarts.
- **Should chitta and presence share a SQLite file?** Argument for: easier inspection, fewer files. Argument against: chitta is Postgres, not SQLite. So no — they stay separate.
- **What does the broadcast inbox look like long-term?** Upstream prunes after 24h. For manas, surfacing "last week's overlap context" might matter. Could extend to longer retention with explicit search, but that starts looking like chitta's job. Keep upstream's 24h default; if cross-session memory matters, route it through chitta as observations.

---

## what this unlocks

- **Tier 1 concurrency item from the review is solved.** `docs/handoff.md` no longer has a silent-overwrite failure mode; /done coordinates via `handoff` lock.
- **/reflect becomes safe to run from anywhere.** User-scope `reflect:user` lock prevents two sessions from corrupting mental-model consolidation.
- **Smriti-scan coordination is free.** Same lock primitive prevents duplicate scans across sessions.
- **Manas-cli `health` and `warm` get richer context** — they can answer "who else is here, what are they doing" without the agent having to figure it out from scratch each session.
- **Foundation for future coordination.** If we later add task assignment, branch reservation, or richer inter-session messaging, the presence subsystem is the natural home — same trust model, same SQLite store, same MCP shape. We don't need to invent the substrate.

---

## next steps

1. **Decide naming** (`presence` vs `sangha`).
2. **Spin up the crate.** Cargo init, copy schema.sql, port the four tool modules. Start with `presence` + `locks`; inbox last.
3. **Write tests first** for the connection-bound identity and the TTL auto-extension — those are the two behaviors that diverge from upstream.
4. **Wire into the manas-architecture doc** — add the "perception: peers" subsystem row and the lock vocabulary table.
5. **Update CLAUDE.md** with the lock vocabulary and the "inbox is advisory, not actionable" rule.
6. **Update the /done skill** to claim/release `handoff`.

Estimate: a focused weekend of work for the core port; another evening for hooks and skill integration. The design is small and the upstream is a clear reference.
