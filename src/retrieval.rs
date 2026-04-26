//! Hybrid retrieval via Reciprocal Rank Fusion (RRF).

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::config::SearchConfig;
use crate::db::{self, SearchHit};
use crate::embedding::EmbedOutput;
use crate::error::Result;
use pgvector::Vector;

#[tracing::instrument(
    name = "retrieval.search_hybrid",
    skip(pool, query_embed, tags, memory_types, query_text, search_cfg),
    fields(k, profile),
)]
pub async fn search_hybrid(
    pool: &PgPool,
    profile: &str,
    query_embed: &EmbedOutput,
    k: i64,
    tags: &[String],
    memory_types: &[String],
    recency_weight: f32,
    recency_half_life_days: f32,
    search_cfg: &SearchConfig,
    query_text: &str,
) -> Result<(Vec<SearchHit>, i64)> {
    let fetch_limit = k * search_cfg.rrf_candidates;
    let query_vec = Vector::from(query_embed.dense.clone());

    // Dense leg (always on): recency_weight=0 so RRF ranks purely by cosine,
    // min_similarity=0.0 so FTS/sparse candidates aren't pre-filtered out.
    let dense_fut = db::search_by_embedding(
        pool, profile, &query_vec, fetch_limit, tags, memory_types, 0.0, 0.0, recency_half_life_days,
    );

    // FTS leg (optional).
    let fts_fut = async {
        if search_cfg.rrf_fts {
            db::search_by_fts(pool, profile, query_text, fetch_limit, tags, memory_types).await
        } else {
            Ok(vec![])
        }
    };

    let (dense_result, fts_result) = tokio::join!(dense_fut, fts_fut);

    let (dense_hits, total) = dense_result?;
    let fts_ids = fts_result.unwrap_or_else(|e| {
        tracing::warn!("FTS leg failed, continuing with dense only: {e}");
        vec![]
    });

    // Build rank lists: doc -> rank (0-based).
    let mut rrf_scores: HashMap<Uuid, f32> = HashMap::new();
    let k_const = search_cfg.rrf_k as f32;

    for (rank, hit) in dense_hits.iter().enumerate() {
        *rrf_scores.entry(hit.id).or_default() += 1.0 / (k_const + rank as f32);
    }
    for (rank, id) in fts_ids.iter().enumerate() {
        *rrf_scores.entry(*id).or_default() += 1.0 / (k_const + rank as f32);
    }

    // Sparse re-ranking leg: operates on the candidate pool from dense + FTS.
    if search_cfg.rrf_sparse && !query_embed.sparse.is_empty() {
        let candidate_ids: Vec<Uuid> = rrf_scores.keys().copied().collect();
        if !candidate_ids.is_empty() {
            match db::fetch_sparse_embeddings(pool, &candidate_ids).await {
                Ok(sparse_docs) => {
                    let mut sparse_scored: Vec<(Uuid, f32)> = sparse_docs
                        .into_iter()
                        .map(|(id, doc_sparse)| {
                            let dot: f32 = query_embed
                                .sparse
                                .iter()
                                .filter_map(|(tok, qw)| doc_sparse.get(tok).map(|dw| qw * dw))
                                .sum();
                            (id, dot)
                        })
                        .collect();
                    sparse_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    for (rank, (id, _score)) in sparse_scored.iter().enumerate() {
                        *rrf_scores.entry(*id).or_default() += 1.0 / (k_const + rank as f32);
                    }
                }
                Err(e) => {
                    tracing::warn!("Sparse leg failed, continuing without: {e}");
                }
            }
        }
    }

    // Sort by RRF score descending, take top k.
    let mut ranked: Vec<(Uuid, f32)> = rrf_scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(k as usize);

    if ranked.is_empty() {
        return Ok((vec![], total));
    }

    let final_ids: Vec<Uuid> = ranked.iter().map(|(id, _)| *id).collect();
    let score_map: HashMap<Uuid, f32> = ranked.into_iter().collect();

    // Hydrate full SearchHit results and stamp RRF scores.
    let mut hits = db::fetch_search_hits_by_ids(pool, profile, &final_ids).await?;
    for hit in &mut hits {
        if let Some(&score) = score_map.get(&hit.id) {
            hit.similarity = score;
        }
    }

    // Apply recency re-ranking post-fusion if enabled.
    if recency_weight > 0.0 {
        let now = chrono::Utc::now();
        let hl_secs = (recency_half_life_days as f64) * 86400.0;
        for hit in &mut hits {
            let age_secs = (now - hit.event_time).num_seconds().max(0) as f64;
            let recency_factor = (-age_secs / hl_secs).exp() as f32;
            hit.similarity *= 1.0 + recency_weight * recency_factor;
        }
        hits.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    }

    Ok((hits, total))
}

#[cfg(test)]
mod tests {
    fn rrf_score(k: u32, ranks: &[usize]) -> f32 {
        ranks.iter().map(|&r| 1.0 / (k as f32 + r as f32)).sum()
    }

    #[test]
    fn rrf_formula_basic() {
        // Document at rank 0 in two legs with k=60.
        let score = rrf_score(60, &[0, 0]);
        let expected = 2.0 / 60.0;
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_formula_different_ranks() {
        // Rank 0 in dense, rank 5 in FTS, k=60.
        let score = rrf_score(60, &[0, 5]);
        let expected = 1.0 / 60.0 + 1.0 / 65.0;
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_single_leg_identity() {
        // Document in only one leg should still get a score.
        let score = rrf_score(60, &[3]);
        let expected = 1.0 / 63.0;
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_higher_rank_beats_lower() {
        // Doc at rank 0 in one leg should outscore doc at rank 10 in one leg.
        let score_top = rrf_score(60, &[0]);
        let score_bottom = rrf_score(60, &[10]);
        assert!(score_top > score_bottom);
    }

    #[test]
    fn rrf_two_legs_beats_one() {
        // Doc in two legs (both at rank 5) should outscore doc in one leg at rank 0.
        let two_legs = rrf_score(60, &[5, 5]);
        let one_leg = rrf_score(60, &[0]);
        assert!(two_legs > one_leg);
    }
}
