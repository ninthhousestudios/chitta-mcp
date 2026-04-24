-- Full-text search (tsvector) and sparse embedding support for hybrid retrieval.

ALTER TABLE memories
    ADD COLUMN content_tsvector tsvector
        GENERATED ALWAYS AS (to_tsvector('english', content)) STORED;

CREATE INDEX idx_memories_content_tsvector ON memories USING GIN (content_tsvector);

ALTER TABLE memories
    ADD COLUMN sparse_embedding jsonb DEFAULT NULL;
