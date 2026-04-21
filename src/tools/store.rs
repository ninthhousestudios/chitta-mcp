//! `store_memory` handler.
//!
//! Validate → (look up by idempotency_key) → embed → insert → return.
//! On `(profile, idempotency_key)` conflict, returns the prior row with
//! `idempotent_replay: true` and does no new work (Principle 6).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pgvector::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::{self, MemoryRow};
use crate::embedding::Embedder;
use crate::error::Result;
use crate::tools::validate;

const TOOL: &str = "store_memory";

/// Arguments for `store_memory`. Derives `JsonSchema` so rmcp can publish the
/// input schema on the wire — single source of truth for CLI and MCP.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct StoreArgs {
    /// Target profile namespace. 1-128 chars, `[a-zA-Z0-9_-]+` only.
    pub profile: String,
    /// Verbatim memory text. Stored as-is (Principle 1).
    pub content: String,
    /// Client-supplied dedup key. Same `(profile, idempotency_key)` returns
    /// the prior row with `idempotent_replay=true`.
    pub idempotency_key: String,
    /// When the subject happened. ISO-8601. Defaults to `record_time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_time: Option<DateTime<Utc>>,
    /// Optional tags. Up to 32, each 1-64 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct StoreOutput {
    pub id: Uuid,
    pub profile: String,
    pub content: String,
    pub event_time: DateTime<Utc>,
    pub record_time: DateTime<Utc>,
    pub tags: Vec<String>,
    pub idempotent_replay: bool,
}

#[tracing::instrument(
    name = "tool.store_memory",
    skip(pool, embedder, args),
    fields(profile = %args.profile, content_len = args.content.len()),
)]
pub async fn handle(
    pool: &PgPool,
    embedder: Arc<Embedder>,
    args: StoreArgs,
) -> Result<StoreOutput> {
    validate::profile(TOOL, &args.profile)?;
    validate::content_non_empty(TOOL, &args.content)?;
    validate::idempotency_key(TOOL, &args.idempotency_key)?;
    if let Some(et) = args.event_time {
        validate::event_time(TOOL, et)?;
    }
    let tags = args.tags.unwrap_or_default();
    validate::tags(TOOL, &tags)?;

    // No fast-path replay check: the unique index on (profile,
    // idempotency_key) is the source of truth, and `insert_or_fetch_memory`
    // handles 23505 by returning the prior row. On the common cold-write
    // path, a pre-flight SELECT costs a pointless round-trip; on replay we
    // pay the embedding cost we would have paid anyway. Correctness lives
    // in the constraint, not in the handler.

    // Embedding is CPU-bound; hand it to spawn_blocking so we do not block
    // the tokio worker for multi-millisecond stretches. We move `content`
    // into the closure and return it alongside the embedding so we neither
    // clone the string nor keep the whole `args` alive across the await.
    let embedder_clone = embedder.clone();
    let content_owned = args.content;
    let (content, embedding_vec) = tokio::task::spawn_blocking(move || {
        let emb = embedder_clone.embed(&content_owned, "store_memory");
        (content_owned, emb)
    })
    .await
    .map_err(|e| crate::error::ChittaError::Internal(format!("spawn_blocking failed: {e}")))?;
    let embedding_vec = embedding_vec?;

    let now = Utc::now();
    let event_time = args.event_time.unwrap_or(now);
    let row = MemoryRow {
        id: Uuid::now_v7(),
        profile: args.profile,
        content,
        embedding: Vector::from(embedding_vec),
        event_time,
        record_time: now,
        tags,
        idempotency_key: args.idempotency_key,
    };

    let (stored, replayed) = db::insert_or_fetch_memory(pool, &row).await?;
    Ok(row_to_output(stored, replayed))
}

fn row_to_output(row: MemoryRow, replayed: bool) -> StoreOutput {
    StoreOutput {
        id: row.id,
        profile: row.profile,
        content: row.content,
        event_time: row.event_time,
        record_time: row.record_time,
        tags: row.tags,
        idempotent_replay: replayed,
    }
}
