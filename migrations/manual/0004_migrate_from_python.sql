-- One-shot data migration from Python chitta (database: chitta) to chitta-rs.
-- Run manually: psql -d chitta_rs -f migrations/0004_migrate_from_python.sql
-- Requires: CREATE EXTENSION IF NOT EXISTS dblink;
--
-- Mapping:
--   id            → id (preserved)
--   profile       → profile
--   content       → content
--   embedding     → embedding (same 1024-dim, same model)
--   created_at    → event_time AND record_time
--   tags          → tags (coalesce null to '{}')
--   idempotency   → 'migrated-' || id::text (synthetic, unique per row)
--   source        → source
--   metadata      → metadata
--
-- Skips rows already present (ON CONFLICT DO NOTHING on primary key).

INSERT INTO memories (id, profile, content, embedding, event_time, record_time, tags, idempotency_key, source, metadata)
SELECT
    src.id,
    src.profile,
    src.content,
    emb.vec,
    src.created_at,                    -- event_time  = created_at
    src.created_at,                    -- record_time = created_at
    COALESCE(src.tags, '{}'),
    'migrated-' || src.id::text,       -- synthetic idempotency_key
    src.source,
    src.metadata::jsonb
FROM dblink(
    'dbname=chitta',
    $$
    SELECT
        id::text,
        profile,
        content,
        embedding::text,
        created_at,
        tags,
        source,
        metadata::text
    FROM memories
    WHERE embedding IS NOT NULL
    $$
) AS src(
    id         uuid,
    profile    text,
    content    text,
    embedding  text,
    created_at timestamptz,
    tags       text[],
    source     text,
    metadata   text
)
CROSS JOIN LATERAL (SELECT src.embedding::vector(1024) AS vec) AS emb
ON CONFLICT (id) DO NOTHING;
