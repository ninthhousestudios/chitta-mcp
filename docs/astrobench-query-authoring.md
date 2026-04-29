# astrobench query authoring guide

Status: in progress
Purpose: spec for what Josh writes when authoring the astrobench query set. Pairs with `docs/plans/astrology-benchmark-plan.md`.

## File layout

One JSONL per slice under `bench/datasets/astrobench/queries/`:

```
slice-a.jsonl   ~30 lines  Sanskrit / jargon
slice-b.jsonl   ~20 lines  exact identifier
slice-c.jsonl   ~15 lines  natural language
slice-d.jsonl   ~15 lines  multi-hop / relational
```

## Line format

```json
{"id": "a-001", "slice": "a", "query": "...", "gold_chunk_ids": ["..."], "notes": "..."}
```

| Field | Meaning |
|---|---|
| `id` | `<slice>-<3-digit>`, e.g. `a-001`, `d-007` |
| `slice` | one of `a`, `b`, `c`, `d` |
| `query` | the question text as an agent would phrase it |
| `gold_chunk_ids` | chitta memory IDs that *should* be retrieved. Slices A/B/C: usually 1. Slice D: 2+ |
| `notes` | free text. Useful for future-you to remember why this query is in the set |

## Authoring workflow

1. Write the query text first.
2. After ingest, in a session connected to `chitta_astrobench`, run `search_memories(profile="astro", query=<your query>, k=30)`.
3. Look through results. Do *you* see the chunk(s) that should answer the query? If not, run a more direct search to find them.
4. Record their memory IDs in `gold_chunk_ids`.
5. **If you can't find a chunk that answers the query at all, drop the query.** Bench queries with no gold answer pollute recall numbers and are worse than no query.

Do this in one or two focused sittings — context-switching costs are high.

## Slice A — Sanskrit / jargon recall

Goal: queries an agent would form using specialized astrology vocabulary.

**Each query must contain at least one of:**

- a Sanskrit term with diacritics (`ā`, `ī`, `ū`, `ṛ`, `ṣ`, `ś`, `ṭ`, `ḍ`, `ṇ`, `ñ`, `ṅ`, `ḷ`, `ḥ`, `ṃ`)
- domain jargon unlikely to appear in general English: `lajjitaadi avastha`, `parivartana yoga`, `argala`, `tajika prashna`, `ashtakavarga`, `vargas`, `dṛṣṭi`, etc.

**Phrase like an agent**: "what do my notes say about X?", "where is X discussed?", "find notes on X in the context of Y."

**Avoid:**
- bare-term queries with no question structure (`"uttarabhādrapadā"`) — that tests term lookup, not retrieval on agent queries
- 25 nakshatra queries — spread across techniques (nakshatras, yogas, dasas, avasthas, varga charts, tajika, prashna, …)

## Slice B — exact-identifier recall

Goal: each query has a single correct chunk because some token in the query appears exactly once in the corpus.

**Workflow:**
1. Run `bench/astrobench/find-unique-tokens.py`. It outputs candidates — tokens that occur exactly once.
2. Pick ~20 candidates that are *meaningful* identifiers (chart positions, dates, specific degrees, named-chart IDs). Skip noise (typos, OCR artifacts, accidental hapaxes).
3. Form a natural question around each.

**Examples:**
- "which chart has Saturn at 27 degrees 15 minutes 28 seconds?"
- "where is the rectification noting Mars in uttarāṣāḍhā 10.03?"
- "find the prashna chart cast on 2024-08-12 18:42 UTC"

**Phrase as a question, not a string match.** Agents ask "find the chart with Saturn at 27 15 28," not `27:15:28`.

## Slice C — natural-language recall

Goal: plain-English questions in lay vocabulary. Tests whether hybrid retrieval *hurts* the cases where dense should already win.

**Avoid specialized terms.** Substitute lay language: "moving backward and close to the sun" not "vakra and asta."

**Each query must be answerable from one specific note**, not from generic astrology knowledge. If a layperson could answer the question without your notes, it's a bad bench query — we're testing retrieval, not LLM world-knowledge.

**Examples:**
- "how should I think about a planet that's both moving backward and close to the sun?"
- "what makes a strong second house in my reading?"
- "what does the course say about predicting career changes?"

## Slice D — multi-hop / relational recall (KG-decision slice)

Goal: queries that need 2+ notes to answer well. The slice that decides whether retrieval has hit a structural ceiling.

**Each query must require synthesis across notes** — not one chunk that mentions both things, but two chunks that *together* form the answer. Information physically separated across files is the test.

**Patterns to use:**
- "what connects X to Y?" — where X and Y live in different notes
- "how does the course's treatment of X compare to its treatment of Y?"
- "which techniques use both X and Y?"
- "where do my notes contradict / agree about Z?"

**Gold set is 2-N chunks.** For each query, identify *every* chunk that contributes material to the answer. Be honest — if you only know of two but suspect a third, search for it before finalizing.

**Avoid:**
- queries answerable by one chunk that lists everything ("what are the nine planets" — one chunk has all nine, this is not multi-hop)
- queries that resolve to a single summary chunk that already synthesized the answer

**Examples:**
- "where do my notes link a planet's lajjitaadi state to predictions about that planet's house?"
- "how does the parivartana yoga course compare to the dasa course on timing?"
- "which notes connect a Saturn–Mars argala to professional-life predictions?"

## Sanity check before handing off

Before finalizing each slice file, verify:

- [ ] every line parses as JSON
- [ ] every `gold_chunk_ids` list is non-empty
- [ ] no duplicate query IDs
- [ ] slice D entries have ≥2 gold chunks
- [ ] you've actually retrieved the gold chunks at least once during authoring (not guessed at IDs)
