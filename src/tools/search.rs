//! `search_memories` handler.
//!
//! Semantic similarity with tag-OR filter and min-similarity floor.
//! Returns the standard envelope; results carry 200-char verbatim snippets
//! (full content only via `get_memory`).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pgvector::Vector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::embedding::Embedder;
use crate::envelope::{Envelope, estimate_tokens};
use crate::error::{ChittaError, Result};
use crate::tools::validate;

const TOOL: &str = "search_memories";

/// Default `k` when the caller does not set one.
const DEFAULT_K: i64 = 10;

/// Snippet length in chars (not bytes). Verbatim prefix; no ellipsis.
const SNIPPET_CHARS: usize = 200;

/// Arguments for `search_memories`. `JsonSchema` is derived so rmcp exposes
/// the same shape callers use on the wire.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchArgs {
    /// Profile scope.
    pub profile: String,
    /// Natural-language query.
    pub query: String,
    /// Max number of results. Default 10; hard cap `validate::MAX_K`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<i64>,
    /// Stop adding results once `budget_spent_tokens` would exceed this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// OR-match: a memory matches if it has any of these tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Cosine-similarity floor in `[0.0, 1.0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    let k = args.k.unwrap_or(DEFAULT_K);
    validate::k(TOOL, k)?;
    let max_tokens = args.max_tokens;
    if let Some(cap) = max_tokens {
        validate::max_tokens(TOOL, cap)?;
    }
    let tags = args.tags.unwrap_or_default();
    validate::tags(TOOL, &tags)?;
    let min_similarity = args.min_similarity.unwrap_or(0.0);
    validate::min_similarity(TOOL, min_similarity)?;

    // Embedding off the async worker.
    let embedder_clone = embedder.clone();
    let query_text = args.query.clone();
    let embedding_vec = tokio::task::spawn_blocking(move || embedder_clone.embed(&query_text))
        .await
        .map_err(|e| ChittaError::Internal(format!("spawn_blocking failed: {e}")))??;

    let query_vec = Vector::from(embedding_vec);

    let (hits, total_available) =
        db::search_by_embedding(pool, &args.profile, &query_vec, k, &tags, min_similarity).await?;

    let candidates: Vec<SearchHit> = hits
        .into_iter()
        .map(|hit| SearchHit {
            id: hit.id,
            snippet: prefix_chars(&hit.content, SNIPPET_CHARS),
            similarity: hit.similarity,
            event_time: hit.event_time,
            record_time: hit.record_time,
            tags: hit.tags,
        })
        .collect();

    let (results, mut truncated) = apply_budget(candidates, max_tokens);

    // `k` bound is applied by SQL `limit`, but if the DB returned fewer rows
    // than `total_available`, that was a hard `k` cut — flag `truncated`.
    if !truncated && (results.len() as i64) < total_available {
        truncated = true;
    }

    // Build the envelope, then overwrite `budget_spent_tokens` with the
    // estimator applied to the fully-assembled payload. This avoids a
    // magic-constant overhead and matches what the wire will carry.
    let mut envelope = Envelope::new(results, truncated, Some(total_available as u64), 0);
    envelope.budget_spent_tokens = estimate_tokens(&envelope);
    Ok(envelope)
}

/// Truncate `candidates` to fit `max_tokens`.
///
/// Returns `(results, truncated)`. When `max_tokens` is `None`, every
/// candidate is kept and `truncated` is `false`. When set, we include the
/// first candidate unconditionally (an empty envelope is less useful than an
/// oversize first result — callers that want strict budgeting should pass a
/// cap large enough to fit at least one hit), then stop adding once the next
/// candidate would push the running token count over `cap`.
fn apply_budget(candidates: Vec<SearchHit>, max_tokens: Option<u64>) -> (Vec<SearchHit>, bool) {
    let Some(cap) = max_tokens else {
        return (candidates, false);
    };
    let mut results: Vec<SearchHit> = Vec::with_capacity(candidates.len());
    let mut spent: u64 = 0;
    let mut truncated = false;
    for candidate in candidates {
        let candidate_tokens = estimate_tokens(&candidate);
        if !results.is_empty() && spent.saturating_add(candidate_tokens) > cap {
            truncated = true;
            break;
        }
        spent = spent.saturating_add(candidate_tokens);
        results.push(candidate);
    }
    (results, truncated)
}

/// Verbatim char-prefix of `s` up to `max_chars` Unicode scalar values.
/// No ellipsis; if `s` is shorter, returns it unchanged.
fn prefix_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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

    fn hit(snippet: &str) -> SearchHit {
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap();
        SearchHit {
            id: Uuid::now_v7(),
            snippet: snippet.to_string(),
            similarity: 0.9,
            event_time: t,
            record_time: t,
            tags: vec![],
        }
    }

    #[test]
    fn apply_budget_none_keeps_all() {
        let candidates = vec![hit("a"), hit("b"), hit("c")];
        let (results, truncated) = apply_budget(candidates, None);
        assert_eq!(results.len(), 3);
        assert!(!truncated);
    }

    #[test]
    fn apply_budget_tight_keeps_at_least_one() {
        // Any single hit is larger than 1 token. We still get the first.
        let (results, truncated) = apply_budget(vec![hit("first"), hit("second")], Some(1));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "first");
        assert!(truncated);
    }

    #[test]
    fn apply_budget_stops_before_overflow() {
        // Estimate each hit, pick a cap that fits exactly one.
        let h1 = hit("one");
        let h2 = hit("two");
        let per = estimate_tokens(&h1);
        let cap = per; // fits the first, blocks the second
        let (results, truncated) = apply_budget(vec![h1, h2], Some(cap));
        assert_eq!(results.len(), 1);
        assert!(truncated);
    }

    #[test]
    fn apply_budget_fits_full_list_when_cap_is_ample() {
        let candidates = vec![hit("a"), hit("b"), hit("c")];
        let total: u64 = candidates.iter().map(estimate_tokens).sum();
        let (results, truncated) = apply_budget(candidates, Some(total + 100));
        assert_eq!(results.len(), 3);
        assert!(!truncated);
    }
}
