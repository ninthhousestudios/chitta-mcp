-- chitta-rs v0.0.1 initial schema.
-- Governed by rust/docs/starting-shape.md § Database and rust/docs/principles.md
-- (principles 1, 2, 6, 7). Change only via a new migration file.

create extension if not exists vector;

create table memories (
    id                uuid         primary key,
    profile           text         not null,
    content           text         not null,
    embedding         vector(1024) not null,
    event_time        timestamptz  not null,
    record_time       timestamptz  not null default now(),
    tags              text[]       not null default '{}',
    idempotency_key   text         not null
);

-- ANN search on embeddings. HNSW with cosine distance.
create index memories_embedding_idx
    on memories using hnsw (embedding vector_cosine_ops)
    with (m = 16, ef_construction = 64);

-- Profile-scoped recent-first listing (and record_time-ordered queries).
create index memories_profile_record_time_idx
    on memories (profile, record_time desc);

-- Tag filtering (OR-match on any tag in `tags`).
create index memories_tags_idx
    on memories using gin (tags);

-- Idempotency: one idempotency_key per profile. The unique constraint
-- IS the dedup mechanism; the write path relies on 23505 to detect replays.
create unique index memories_profile_idempotency_key_uniq
    on memories (profile, idempotency_key);
