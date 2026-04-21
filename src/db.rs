//! Postgres + pgvector repo.
//!
//! Runtime-checked queries (`sqlx::query`/`query_as`) so a fresh clone
//! can `cargo build` without a live database — rationale in
//! `docs/starting-shape.md` § sqlx mode.

use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::FromRow;
use sqlx::postgres::{PgPoolOptions, PgPool};
use uuid::Uuid;

use crate::config::Config;
use crate::error::{ChittaError, Result};

/// One row of `memories`. Mirrors the schema in `migrations/0001_init.sql`.
#[derive(Debug, Clone, FromRow)]
pub struct MemoryRow {
    pub id: Uuid,
    pub profile: String,
    pub content: String,
    pub embedding: Vector,
    pub event_time: DateTime<Utc>,
    pub record_time: DateTime<Utc>,
    pub tags: Vec<String>,
    pub idempotency_key: String,
}

/// One hit from an ANN search. Similarity is `1 - cosine_distance`.
#[derive(Debug, Clone, FromRow)]
pub struct SearchHit {
    pub id: Uuid,
    pub content: String,
    pub event_time: DateTime<Utc>,
    pub record_time: DateTime<Utc>,
    pub tags: Vec<String>,
    pub similarity: f32,
}

pub async fn connect(cfg: &Config) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.db_max_connections)
        .acquire_timeout(std::time::Duration::from_secs(cfg.db_acquire_timeout_secs))
        .idle_timeout(std::time::Duration::from_secs(cfg.db_idle_timeout_secs))
        .connect(&cfg.database_url)
        .await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// The SQLSTATE code Postgres raises on unique-constraint violation.
/// We intercept it on `insert_memory` to implement the idempotency contract.
const PG_UNIQUE_VIOLATION: &str = "23505";

/// Attempt to insert. On `(profile, idempotency_key)` conflict, fetch and
/// return the existing row — this is the idempotency contract (Principle 6).
///
/// Returns `(row, idempotent_replay)`.
pub async fn insert_or_fetch_memory(
    pool: &PgPool,
    new: &MemoryRow,
) -> Result<(MemoryRow, bool)> {
    let insert_result = sqlx::query_as::<_, MemoryRow>(
        r#"
        insert into memories
            (id, profile, content, embedding, event_time, record_time, tags, idempotency_key)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        returning id, profile, content, embedding, event_time, record_time, tags, idempotency_key
        "#,
    )
    .bind(new.id)
    .bind(&new.profile)
    .bind(&new.content)
    .bind(&new.embedding)
    .bind(new.event_time)
    .bind(new.record_time)
    .bind(&new.tags)
    .bind(&new.idempotency_key)
    .fetch_one(pool)
    .await;

    match insert_result {
        Ok(row) => Ok((row, false)),
        Err(e) => {
            if is_unique_violation(&e) {
                let existing = find_by_idempotency_key(pool, &new.profile, &new.idempotency_key)
                    .await?
                    .ok_or_else(|| {
                        ChittaError::Internal(
                            "unique violation without recoverable row".to_string(),
                        )
                    })?;
                Ok((existing, true))
            } else {
                Err(e.into())
            }
        }
    }
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = e {
        db.code().as_deref() == Some(PG_UNIQUE_VIOLATION)
    } else {
        false
    }
}

pub async fn find_by_idempotency_key(
    pool: &PgPool,
    profile: &str,
    idempotency_key: &str,
) -> Result<Option<MemoryRow>> {
    let row = sqlx::query_as::<_, MemoryRow>(
        r#"
        select id, profile, content, embedding, event_time, record_time, tags, idempotency_key
        from memories
        where profile = $1 and idempotency_key = $2
        "#,
    )
    .bind(profile)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn get_memory_by_id(
    pool: &PgPool,
    profile: &str,
    id: Uuid,
) -> Result<Option<MemoryRow>> {
    let row = sqlx::query_as::<_, MemoryRow>(
        r#"
        select id, profile, content, embedding, event_time, record_time, tags, idempotency_key
        from memories
        where profile = $1 and id = $2
        "#,
    )
    .bind(profile)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Minimum `hnsw.ef_search` used for every semantic query. pgvector's
/// default is 40, which both caps HNSW candidate breadth and undershoots
/// any WHERE post-filter that rejects most of those candidates. We raise
/// the floor so (a) `min_similarity`/tag filters don't silently shrink
/// result counts and (b) `LIMIT k` actually returns ~k rows when matches
/// exist. Capped at `HNSW_EF_SEARCH_MAX` to bound per-query work.
const HNSW_EF_SEARCH_MIN: i64 = 200;
const HNSW_EF_SEARCH_MAX: i64 = 1000;

/// Semantic search with optional tag filter and similarity floor.
///
/// Tag match is OR: a row passes if it shares at least one tag with `tags`.
/// When `tags` is empty, no tag filter is applied.
///
/// Returns `(hits, total_available)`. `total_available` is the count of rows
/// matching **profile + tag filter** — it deliberately ignores
/// `min_similarity`, because counting rows above a cosine threshold would
/// require scanning every embedding, defeating the ANN index. The agent gets
/// a truthful ceiling on candidate breadth; the similarity-gated subset is
/// what `results` reports.
///
/// Runs inside a short transaction so `SET LOCAL hnsw.ef_search` scopes to
/// the ANN query only and doesn't leak to other pool users.
pub async fn search_by_embedding(
    pool: &PgPool,
    profile: &str,
    query: &Vector,
    k: i64,
    tags: &[String],
    min_similarity: f32,
) -> Result<(Vec<SearchHit>, i64)> {
    // Cheap pre-count under the same filters we expose to the caller.
    // No distance term here — it's a sequential filter count, bounded by
    // the size of the profile.
    let total: i64 = sqlx::query_scalar(
        r#"
        select count(*)::bigint
        from memories
        where profile = $1
          and ($2::text[] = '{}' or tags && $2)
        "#,
    )
    .bind(profile)
    .bind(tags)
    .fetch_one(pool)
    .await?;

    // ef_search is an integer GUC; SET LOCAL does not accept bind params,
    // so we clamp to a known-safe integer range and format inline. k is
    // already range-checked by the validator; the clamp below is belt +
    // suspenders against a future caller reaching this fn with a bad k.
    let ef_search = (k.max(1) * 4).clamp(HNSW_EF_SEARCH_MIN, HNSW_EF_SEARCH_MAX);
    let mut tx = pool.begin().await?;
    sqlx::query(&format!("set local hnsw.ef_search = {ef_search}"))
        .execute(&mut *tx)
        .await?;

    // `1 - (embedding <=> $2)::real` gives cosine similarity in [0, 1] for
    // L2-normalized vectors. We filter on it directly so HNSW still drives
    // the ordering.
    let hits = sqlx::query_as::<_, SearchHit>(
        r#"
        select
            id,
            content,
            event_time,
            record_time,
            tags,
            (1.0 - (embedding <=> $2))::real as similarity
        from memories
        where profile = $1
          and ($3::text[] = '{}' or tags && $3)
          and (1.0 - (embedding <=> $2))::real >= $4
        order by embedding <=> $2
        limit $5
        "#,
    )
    .bind(profile)
    .bind(query)
    .bind(tags)
    .bind(min_similarity)
    .bind(k)
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((hits, total))
}
