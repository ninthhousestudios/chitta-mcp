# astrobench — astrology retrieval benchmark

Status: build plan
Date: 2026-04-27
Purpose: small retrieval benchmark tailored to chitta's real workload (specialized vocabulary, structured identifiers, relational queries). Answers two questions:

1. Does dense+sparse hybrid beat dense-only on identifier-heavy and jargon queries?
2. Does retrieval (any config) plateau on multi-hop / relational queries — i.e., is a knowledge graph justified, or would multi-query retrieval close the gap first?

PersonaMem's rrf-sweep showed sparse adds +2.9pp recall@20 on natural-language conversational memory. End-to-end RAG accuracy didn't move because the answer LLM was the bottleneck. Astrobench measures retrieval directly on a corpus whose lexical shape matches chitta's actual workload.

## Why `~/vault/astro/` is a good fit

- **470 markdown files, ~2.2MB total**, ~4.6KB average — enough for stable recall@k without expensive indexing.
- **Sanskrit transliteration with diacritics** (`uttarabhādrapadā`, `kṛttikā`, `mṛgaśīrṣa`): exactly where dense BPE tokenizers fragment unpredictably while sparse weights preserve distinct lexical tokens.
- **Structured chart identifiers** (`Saturn 27:15:28 uttarabhādrapadā 4.87`): degree-minute-second + nakshatra + pada quartets that uniquely identify positions. Dense smears these; sparse can pin them.
- **Specialized concept vocabulary** (`Lajjitaadi Avasthas`, `Parivartana Yogas`, `Tajika Prashna`, `Ashtakavarga`, `Argala`): jargon underrepresented in BGE-M3's general training distribution.

These three properties are good proxies for chitta's real workload (file paths, symbol names, error strings) without requiring a synthetic identifier corpus.

## Corpus and database

| Profile | Source | Files | Role |
|---|---|---|---|
| `astro` | `~/vault/astro/` | 470 | primary — all queries scoped here |
| `iching` | `~/vault/iching/` | 14 | distractor in same DB |
| `cards` | `~/vault/cards/` | 238 | distractor in same DB |

All three ingested into a dedicated Postgres database `chitta_astrobench` (separate from any production chitta DB), each in its own profile. iching and cards are present as cross-profile distractors — queries are scoped to `astro`, so they should not appear in results. If they do, that's a profile-isolation bug the bench will surface.

**Chunking**: 512 tokens, 64 overlap. Matches the personamem rrf-sweep config for comparability.

## Build once, test many

The ingest is the slow step. To make this reusable so we can iterate on configs and queries cheaply:

1. Provision `chitta_astrobench` with chitta-rs schema (migrations).
2. Run `bench/astrobench/ingest.py` (to be written): chunk → dense+sparse embeddings → idempotent insert per profile, keyed on content hash so reruns skip existing chunks.
3. Snapshot when ingest completes: `pg_dump -Fc chitta_astrobench > bench/datasets/astrobench/snapshot-<date>.dump`. Local-only, not committed (size + contains corpus content). Regenerable by rerunning ingest.
4. All subsequent runs read from the DB. If it's wiped, restore from the dump in minutes instead of re-embedding for hours.

Wall-clock estimate for ingest: hours, not days. Acceptable as a one-time cost.

## Query slices

Each slice is a JSONL file under `bench/datasets/astrobench/queries/`. Authoring guidance and gold-resolution workflow live in `docs/astrobench-query-authoring.md`.

### Slice A — Sanskrit / jargon recall (~30 queries, hand-written)

Queries containing diacritic-bearing terms or domain jargon. Tests OOV-jargon recall and diacritic handling. Gold: 1 chunk per query.

### Slice B — exact-identifier recall (~20 queries, helper-assisted)

Each query targets a chunk via a token that appears exactly once in the corpus. A helper script `bench/astrobench/find-unique-tokens.py` produces candidate tokens; Josh picks ~20 meaningful ones and writes a question around each. Gold: 1 chunk per query.

### Slice C — natural-language recall (~15 queries, hand-written)

Plain-English questions without specialized vocabulary. Tests whether hybrid hurts on the cases where dense already wins. Gold: 1 chunk per query.

### Slice D — multi-hop / relational recall (~15 queries, hand-written) — KG-decision slice

Queries that require synthesizing across 2+ notes. Concepts that connect, contrast, or build on each other physically separated in the corpus. Gold: 2-N chunks per query.

**This slice is the load-bearing one for the KG-vs-multi-query decision.** Slices A/B/C measure lexical-precision tradeoffs; only D probes whether single-query retrieval has a structural ceiling on relational reasoning.

## Configs

| Config | Legs | rrf_k |
|---|---|---|
| dense-only | dense | — |
| dense+sparse | dense + BGE-M3 sparse | 60 |
| dense+sparse+fts | dense + sparse + Postgres FTS | 60 |

No rrf_k sweep on first pass — personamem showed it barely matters.

## Metrics

- recall@5, recall@10, recall@20
- per-slice breakdown (the whole point of slicing)
- retrieve latency (sanity check; should be ~flat across configs)
- profile-isolation sanity check: pick 5 slice-A queries, run with and without profile filter, confirm distractor profiles never leak into `astro`-scoped results

No LLM, no end-to-end accuracy. We learned from personamem that LLM end-to-end swamps the retrieval signal.

## Decision matrix

| Result | Reading | Next step |
|---|---|---|
| Slice A: sparse adds >5pp recall@20 | Hybrid wins on jargon | Make hybrid default in chitta-rs |
| Slice B: sparse adds >10pp recall@20 | Hybrid wins on exact identifiers | Same |
| Slice A/B: sparse adds <2pp | Hybrid not worth complexity for this distribution | Document, revisit after multi-query |
| Slice C: sparse hurts >2pp | Hybrid shouldn't be unconditional | Add query-routing logic before shipping |
| Slice D: hybrid recall@20 > 70% | Retrieval is not the structural bottleneck | Multi-query retrieval next; KG deferred |
| Slice D: hybrid recall@20 < 60% | Single-query retrieval has hit a ceiling on relational queries | KG justified — start with memory-link graph |
| Slice D: 60–70% | Ambiguous | Try multi-query first as the cheaper experiment, re-measure |

## Implementation steps

1. Provision `chitta_astrobench` and run chitta-rs migrations. (~1h)
2. Write `bench/astrobench/ingest.py` — chunker + dense+sparse embedder + idempotent per-profile insert. (~3h)
3. Run ingest on astro + iching + cards. (~hours wall-clock, mostly embeddings)
4. `pg_dump` snapshot. (~10min)
5. Write `bench/astrobench/find-unique-tokens.py` for slice B candidates. (~1h)
6. **Josh authors queries** — slices A/C/D hand-written, slice B from helper output. (~2h Josh)
7. **Josh resolves gold chunk IDs** for each query against the bench DB. (~1h Josh)
8. Adapt `bench/retrieval-eval.py` to read astrobench query format and run against `chitta_astrobench`. (~1h)
9. Run all three configs, produce per-slice recall@k report. (~1h compute + analysis)

Total Claude work: ~7h. Total Josh work: ~3h.

## Open question

- How much of the existing `bench/retrieval-eval.py` plumbing is reusable vs needs a fork for the astrobench corpus shape? Will know after step 1.

## What this is NOT

- Not an end-to-end RAG benchmark — personamem covers that.
- Not a multilingual benchmark, despite Sanskrit content. All queries are English with Sanskrit terms embedded.
- Not a substitute for a chitta-self benchmark on real session data (file paths, symbol names from actual chitta usage). It's a *proxy* with similar lexical properties; a chitta-self bench is a worthwhile follow-up if astrobench validates the hypothesis.
