# Contract Alignment: chitta-rs vs chitta (Python)

## Decision

**Diverge.** chitta-rs keeps its own field names. The cutover to Rust will update all call sites.

### Rationale

- Rust naming follows Rust conventions (snake_case, idiomatic types)
- The migration window from Python to Rust is short enough that maintaining two naming schemes adds complexity without value
- Call sites are controlled (CLAUDE.md instructions, Claude Code configs) and can be updated atomically at cutover

## Field Differences

| Context | Python Field | Rust Field | Notes |
|---------|-------------|------------|-------|
| `store_memory` input | *(not required)* | `profile` | Required in Rust; was optional/implicit in Python |
| `store_memory` input | *(not required)* | `idempotency_key` | Required in Rust; did not exist in Python |
| `store_memory` input | `source` | *(dropped)* | Python-only; removed in Rust |
| `store_memory` input | `metadata` | *(dropped)* | Python-only; removed in Rust |
| `store_memory` input | `auto_link` | *(dropped)* | Python-only; removed in Rust |
| `store_memory` output | `source` | *(dropped)* | Python-only; not present in Rust response |
| `store_memory` output | `links_created` | *(dropped)* | Python-only; not present in Rust response |
| `store_memory` output | *(absent)* | `event_time` | Rust-only addition |
| `store_memory` output | *(absent)* | `record_time` | Rust-only addition |
| `store_memory` output | *(absent)* | `idempotent_replay` | Rust-only addition |
| `get_memory` input | `memory_id` | `id` | Renamed |
| `get_memory` input | *(optional)* | `profile` | Promoted to required in Rust |
| `get_memory` output | `{memory: null}` (in-band) | JSON-RPC error | Not-found moved out-of-band |
| `search_memories` input | `limit` | `k` | Renamed |
| `search_memories` input | `source` | *(dropped)* | Python-only |
| `search_memories` input | `graph_depth` | *(dropped)* | Python-only |
| `search_memories` input | `profiles` | *(dropped)* | Python used plural; Rust has singular `profile` |
| `search_memories` envelope | `budget_spent` | `budget_spent_tokens` | Renamed for clarity |
| `search_memories` envelope | `refinement_advice` | *(dropped)* | Python-only |
| `search_memories` hit | `score` | `similarity` | Renamed |
| `search_memories` hit | `title` | *(dropped)* | Python-only |
| `search_memories` hit | `entities` | *(dropped)* | Python-only |

## Cutover Checklist

When switching from Python chitta to chitta-rs:

- [ ] Update CLAUDE.md instructions referencing Python field names
- [ ] Update Claude Code MCP server config (type, command/url)
- [ ] Update any agent configs that parse tool outputs
- [ ] Update any scripts that call chitta tools directly
- [ ] Verify all call sites use Rust field names

## Impact on v0.0.3

- Schema mapping tools (if any) must handle the name differences
- v0.0.3 migration tooling should not attempt to bridge Python names
