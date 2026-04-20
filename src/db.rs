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
        .max_connections(8)
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

/// Semantic search with optional tag filter and similarity floor.
///
/// Tag match is OR: a row passes if it shares at least one tag with `tags`.
/// When `tags` is empty, no tag filter is applied.
pub async fn search_by_embedding(
    pool: &PgPool,
    profile: &str,
    query: &Vector,
    k: i64,
    tags: &[String],
    min_similarity: f32,
) -> Result<Vec<SearchHit>> {
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
    .fetch_all(pool)
    .await?;
    Ok(hits)
}

/// Count of rows that would pass the filters, ignoring `k` and token budget.
/// Used for the envelope's `total_available` field.
pub async fn count_matching(
    pool: &PgPool,
    profile: &str,
    query: &Vector,
    tags: &[String],
    min_similarity: f32,
) -> Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        r#"
        select count(*)::bigint
        from memories
        where profile = $1
          and ($3::text[] = '{}' or tags && $3)
          and (1.0 - (embedding <=> $2))::real >= $4
        "#,
    )
    .bind(profile)
    .bind(query)
    .bind(tags)
    .bind(min_similarity)
    .fetch_one(pool)
    .await?;
    Ok(count)
}
