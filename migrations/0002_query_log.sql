CREATE TABLE IF NOT EXISTS query_log (
    id          bigserial    PRIMARY KEY,
    profile     text         NOT NULL,
    query_text  text         NOT NULL,
    embedding   vector(1024) NOT NULL,
    k           integer      NOT NULL,
    min_similarity real      NOT NULL DEFAULT 0.0,
    tags        text[]       NOT NULL DEFAULT '{}',
    result_ids  uuid[]       NOT NULL DEFAULT '{}',
    result_scores real[]     NOT NULL DEFAULT '{}',
    total_available bigint,
    truncated   boolean      NOT NULL DEFAULT false,
    latency_ms  integer      NOT NULL,
    created_at  timestamptz  NOT NULL DEFAULT now()
);
CREATE INDEX query_log_created_at_idx ON query_log (created_at DESC);
CREATE INDEX query_log_profile_idx ON query_log (profile);
