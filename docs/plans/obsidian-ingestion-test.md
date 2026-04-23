# obsidian vault ingestion test

Test case for domain-knowledge ingestion. An astrology teacher's Obsidian
vault — a mix of domain reference knowledge and personal interpretive
knowledge — ingested into Chitta under its own profile.

## Goals

1. **Test ingestion quality** — does Chitta retrieve domain knowledge well
   when it comes from a real Obsidian vault with natural structure?
2. **Test profile isolation** — teacher's knowledge lives in its own profile,
   separate from Josh's personal memories. No cross-contamination.
3. **Test cross-profile search** — agent searches both profiles, merges
   results. Personal patterns + domain reference = richer synthesis.
4. **Establish a domain-knowledge retrieval baseline** — complement
   PersonaMem/BEAM (conversational memory) with a domain-knowledge benchmark.
5. **Repeatable across versions** — run on v0.0.2, then again after v0.0.3
   retrieval improvements, and compare.

## The vault

Astrology notes from a teacher. Content includes:
- Planetary meanings and significations
- House interpretations
- Aspect patterns
- Interpretive frameworks and techniques
- Personal observations and examples from the teacher's practice

This is interesting because it blends reference knowledge ("Saturn rules
Capricorn") with interpretive knowledge ("in my experience, Saturn in the
7th tends to..."). Chitta doesn't need to distinguish these — both are
memories. But retrieval quality may differ between factual lookups and
interpretive queries.

## Profile design

- **Profile name:** TBD (teacher's name or a descriptive label)
- **Separate from `josh`** — this is the teacher's knowledge, not Josh's
- **Source tag:** `source: "obsidian-import"` on all ingested memories
- **Tags derived from:** frontmatter fields, folder path, note title

## Ingestion pipeline

A Python script: `bench/ingest-obsidian.py`

### Step 1: scan

Walk the vault directory. For each `.md` file:
- Skip empties, templates, MOC (map of content) files, daily notes
- Parse YAML frontmatter → tags and metadata
- Extract wiki-links → additional tags (linked concepts)
- Measure token length

Report: total files, skipped files (with reasons), token distribution.

### Step 2: chunk

Two strategies to test:

**Note-level** — each note is one memory. Simple. Works well for short,
atomic notes (which Obsidian encourages). Loses context on long notes.

**Heading-level** — split at h2/h3 boundaries. Each chunk gets the note
title prepended as context. Better for long reference notes that cover
multiple topics. More chunks, more precise retrieval.

Run both and compare retrieval quality on the validation questions.

### Step 3: dry run

Print what would be stored without storing it:
```
Found 240 notes (12 skipped: 5 empty, 4 templates, 3 daily notes)
Note-level: 228 memories, ~38k tokens
Heading-level: 380 memories, ~45k tokens (overhead from title context)

Sample memory:
  profile: ernst
  content: "## Saturn in the 7th House\n\nSaturn here brings..."
  tags: [planets, saturn, houses, 7th-house, relationships]
  source: obsidian-import
  idempotency_key: obsidian:ernst-vault:planets/saturn.md:2
```

Human reviews, adjusts skip rules or tag extraction, re-runs.

### Step 4: store

Call `store_memory` for each chunk:
- `idempotency_key`: `obsidian:{profile}:{filepath}:{chunk_index}`
  (re-runs are safe, same key returns prior result)
- `source`: `"obsidian-import"`
- `tags`: from frontmatter + folder path + wiki-links
- `event_time`: from frontmatter date field if present, otherwise omit
  (defaults to record_time)

Progress bar. Save a manifest of what was stored (filepath → memory ID)
for later reference.

### Step 5: validate

Write 20-30 questions the vault should be able to answer. Mix of:
- **Factual lookups:** "What does Saturn signify?" "Which houses are
  angular?"
- **Interpretive queries:** "How does the teacher interpret Saturn in
  the 7th?" "What patterns does he see with strong Moon placements?"
- **Cross-reference:** "What planets are related to partnerships?"
- **Synthesis:** "What is the teacher's overall approach to reading
  difficult placements?"

Run each query via `search_memories(profile, query, k=10)`. Manually
grade: did the right content surface? Was it in the top 3? Top 5?

Record results as a simple accuracy table. This becomes the baseline
for comparing across chitta versions.

## Cross-profile test

After ingestion, test the confluence:

1. Search `josh` profile: "Saturn in the 7th house"
   → Josh's personal session notes, client observations
2. Search teacher profile: "Saturn in the 7th house"
   → Teacher's interpretive framework, reference knowledge
3. Agent merges both: personal experience + domain framework

This is the cognitive confluence in action. Measure whether the merged
results are qualitatively richer than either alone.

## Script interface

```
# scan + dry run (no writes)
python bench/ingest-obsidian.py \
  --vault ~/path/to/vault \
  --profile ernst \
  --dry-run

# ingest with note-level chunking
python bench/ingest-obsidian.py \
  --vault ~/path/to/vault \
  --profile ernst \
  --chunk-strategy note

# ingest with heading-level chunking
python bench/ingest-obsidian.py \
  --vault ~/path/to/vault \
  --profile ernst \
  --chunk-strategy heading

# validate against question set
python bench/ingest-obsidian.py \
  --profile ernst \
  --validate bench/obsidian-validation-questions.json
```

## What we're not building

- A general-purpose Obsidian sync tool. This is a one-shot test import.
- Live sync / file watcher. If the vault changes, re-run the script
  (idempotency keys make this safe).
- Obsidian plugin. The script reads files directly.
- Anything that needs an LLM on the import path (Principle 9). Tag
  extraction is rule-based: frontmatter, folder names, wiki-links.

## Timeline

Not blocking v0.0.3. Can run independently:
- First run on v0.0.2 to establish baseline
- Re-run after v0.0.3 retrieval improvements to measure lift
- Results feed back into whether domain-knowledge retrieval needs
  different treatment than conversational memory

## Open questions

- What's the right chunk size for Obsidian notes? The vault will tell us.
- Do wiki-links make good tags, or do they add noise?
- Does the teacher's interpretive knowledge retrieve differently than
  factual/reference knowledge? If so, that informs the personal vs
  reference distinction.
- Should cross-profile search be a Chitta feature, or is two calls +
  agent-side merge good enough?
