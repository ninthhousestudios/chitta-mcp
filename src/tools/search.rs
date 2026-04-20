//! `search_memories` handler.
//!
//! Semantic similarity with tag-OR filter and min-similarity floor.
//! Returns the standard envelope; results carry 200-char verbatim snippets
//! (full content only via `get_memory`).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::embedding::Embedder;
use crate::envelope::{Envelope, estimate_tokens};
use crate::error::{ChittaError, Result};
use crate::tools::validate;

const TOOL: &str = "search_memories";

/// Hard cap on `k` — we fetch (k + buffer) candidates so min_similarity
/// filtering upstream of us leaves enough results. v0.0.1 uses k directly.
const DEFAULT_K: i64 = 10;

/// Snippet length in chars (not bytes). Verbatim prefix; no ellipsis.
const SNIPPET_CHARS: usize = 200;

#[derive(Debug, Deserialize)]
pub struct SearchArgs {
    pub profile: String,
    pub query: String,
    #[serde(default)]
    pub k: Option<i64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub min_similarity: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub id: Uuid,
    pub snippet: String,
    pub similarity: f32,
    pub event_time: DateTime<Utc>,
    pub record_time: DateTime<Utc>,
    pub tags: Vec<String>,
}

pub type SearchOutput = Envelope<SearchHit>;

pub async fn handle(
    pool: &PgPool,
    embedder: Arc<Embedder>,
    args: SearchArgs,
) -> Result<SearchOutput> {
    validate::profile(TOOL, &args.profile)?;
    if args.query.is_empty() {
        return Err(ChittaError::InvalidArgument {
            tool: TOOL,
            argument: "query".to_string(),
            constraint: "length >= 1".to_string(),
            received: Some(serde_json::json!("")),
            next_action: "Pass a non-empty query string.".to_string(),
        });
    }

    let k = args.k.unwrap_or(DEFAULT_K).max(1);
    let max_tokens = args.max_tokens;
    let tags = args.tags.unwrap_or_default();
    validate::tags(TOOL, &tags)?;
    let min_similarity = args.min_similarity.unwrap_or(0.0);

    // Embedding off the async worker.
    let embedder_clone = embedder.clone();
    let query_text = args.query.clone();
    let embedding_vec = tokio::task::spawn_blocking(move || embedder_clone.embed(&query_text))
        .await
        .map_err(|e| ChittaError::Internal(format!("spawn_blocking failed: {e}")))??;

    let query_vec = Vector::from(embedding_vec);

    let hits =
        db::search_by_embedding(pool, &args.profile, &query_vec, k, &tags, min_similarity).await?;
    let total_available =
        db::count_matching(pool, &args.profile, &query_vec, &tags, min_similarity).await?;

    // Build results with token-budget truncation.
    let mut results: Vec<SearchHit> = Vec::with_capacity(hits.len());
    let mut spent: u64 = 0;
    let mut truncated = false;
    for hit in hits {
        let snippet = prefix_chars(&hit.content, SNIPPET_CHARS);
        let candidate = SearchHit {
            id: hit.id,
            snippet,
            similarity: hit.similarity,
            event_time: hit.event_time,
            record_time: hit.record_time,
            tags: hit.tags,
        };
        if let Some(cap) = max_tokens {
            let candidate_tokens = estimate_tokens(&candidate);
            if spent + candidate_tokens > cap && !results.is_empty() {
                truncated = true;
                break;
            }
            spent += candidate_tokens;
        } else {
            spent += estimate_tokens(&candidate);
        }
        results.push(candidate);
    }

    // `k` bound is applied by SQL `limit`, but if the DB returned fewer rows
    // than `total_available`, that was a hard `k` cut — flag `truncated`.
    if !truncated && (results.len() as i64) < total_available {
        truncated = true;
    }

    // Envelope overhead — the three scalar fields contribute a handful of
    // bytes; we account the results above and add a small constant here.
    let envelope_overhead = 12; // ~ ceil(48 bytes / 4)
    Ok(Envelope::new(
        results,
        truncated,
        Some(total_available as u64),
        spent + envelope_overhead,
    ))
}

/// Verbatim char-prefix of `s` up to `max_chars` Unicode scalar values.
/// No ellipsis; if `s` is shorter, returns it unchanged.
fn prefix_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_chars_truncates_at_boundary() {
        let s = "abcdefg";
        assert_eq!(prefix_chars(s, 4), "abcd");
    }

    #[test]
    fn prefix_chars_returns_whole_when_short() {
        assert_eq!(prefix_chars("ab", 10), "ab");
    }

    #[test]
    fn prefix_chars_handles_multi_byte_unicode() {
        let s = "αβγδε"; // 5 chars, 10 bytes
        assert_eq!(prefix_chars(s, 3), "αβγ");
    }
}
