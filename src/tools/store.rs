//! `store_memory` handler.
//!
//! Validate → (look up by idempotency_key) → embed → insert → return.
//! On `(profile, idempotency_key)` conflict, returns the prior row with
//! `idempotent_replay: true` and does no new work (Principle 6).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::{self, MemoryRow};
use crate::embedding::Embedder;
use crate::error::Result;
use crate::tools::validate;

const TOOL: &str = "store_memory";

#[derive(Debug, Deserialize)]
pub struct StoreArgs {
    pub profile: String,
    pub content: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub event_time: Option<DateTime<Utc>>,
    #[serde(default)]
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

    // Fast-path replay: avoid embedding if the key is already used.
    // (Not a correctness guarantee — the unique index is the source of
    // truth. This just spares the ONNX pass on the common replay case.)
    if let Some(existing) =
        db::find_by_idempotency_key(pool, &args.profile, &args.idempotency_key).await?
    {
        return Ok(row_to_output(existing, true));
    }

    // Embedding is CPU-bound; hand it to spawn_blocking so we do not block
    // the tokio worker for multi-millisecond stretches.
    let embedder_clone = embedder.clone();
    let content_for_embed = args.content.clone();
    let embedding_vec = tokio::task::spawn_blocking(move || embedder_clone.embed(&content_for_embed))
        .await
        .map_err(|e| crate::error::ChittaError::Internal(format!("spawn_blocking failed: {e}")))??;

    let now = Utc::now();
    let event_time = args.event_time.unwrap_or(now);
    let row = MemoryRow {
        id: Uuid::now_v7(),
        profile: args.profile,
        content: args.content,
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
