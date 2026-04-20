//! rmcp server: three tools, stdio transport.
//!
//! Tool handlers live in [`crate::tools`]; this module wires them to rmcp's
//! `ToolRouter` and maps [`ChittaError`](crate::error::ChittaError) to
//! JSON-RPC errors whose `data` field carries the Principle 8 contract
//! (`tool`, `constraint`, `next_action`).

use std::sync::Arc;

use rmcp::{
    ErrorData, Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::embedding::Embedder;
use crate::error::ChittaError;
use crate::tools;

/// Shared server state. Cheap to clone (Arc + PgPool both clone-cheap).
#[derive(Clone)]
pub struct ChittaServer {
    pool: PgPool,
    embedder: Arc<Embedder>,
    tool_router: ToolRouter<Self>,
}

impl ChittaServer {
    pub fn new(pool: PgPool, embedder: Arc<Embedder>) -> Self {
        Self { pool, embedder, tool_router: Self::tool_router() }
    }
}

// ---- rmcp-visible argument schemas -----------------------------------
//
// rmcp's tool macro uses `schemars::JsonSchema` to generate the tool's
// input schema on the wire. The internal `tools::*Args` types are
// deserialize-only; these mirror them with schemars derives.

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StoreArgs {
    /// Target profile namespace. 1-128 chars, [a-zA-Z0-9_-]+ only.
    pub profile: String,
    /// Verbatim memory text. Stored as-is (Principle 1).
    pub content: String,
    /// Client-supplied dedup key. Same (profile, idempotency_key) returns the prior row.
    pub idempotency_key: String,
    /// When the subject happened. ISO-8601. Defaults to record_time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional tags. Up to 32, each 1-64 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetArgs {
    /// Profile scope.
    pub profile: String,
    /// Memory UUID.
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Profile scope.
    pub profile: String,
    /// Natural-language query.
    pub query: String,
    /// Max number of results. Default 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<i64>,
    /// Stop adding results once `budget_spent_tokens` would exceed this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// OR-match: a memory matches if it has any of these tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Cosine-similarity floor in [0.0, 1.0].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_similarity: Option<f32>,
}

// ---- Tool handlers ---------------------------------------------------

#[tool_router(router = tool_router)]
impl ChittaServer {
    /// Store a new memory. Idempotent on (profile, idempotency_key).
    #[tool(description = "Store a new memory. Idempotent on (profile, idempotency_key): \
                          resubmitting the same key returns the prior row with \
                          idempotent_replay=true.")]
    pub async fn store_memory(
        &self,
        Parameters(args): Parameters<StoreArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let inner = tools::StoreArgs {
            profile: args.profile,
            content: args.content,
            idempotency_key: args.idempotency_key,
            event_time: args.event_time,
            tags: args.tags,
        };
        let out = tools::store::handle(&self.pool, self.embedder.clone(), inner)
            .await
            .map_err(chitta_to_rmcp)?;
        let v = serde_json::to_value(&out).map_err(json_to_rmcp)?;
        Ok(Json(v))
    }

    /// Fetch a memory by id.
    #[tool(description = "Fetch a memory by profile + id. Returns the full row. \
                          Errors with not_found if the id is unknown in that profile.")]
    pub async fn get_memory(
        &self,
        Parameters(args): Parameters<GetArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let inner = tools::GetArgs { profile: args.profile, id: args.id };
        let out = tools::get::handle(&self.pool, inner).await.map_err(chitta_to_rmcp)?;
        let v = serde_json::to_value(&out).map_err(json_to_rmcp)?;
        Ok(Json(v))
    }

    /// Semantic similarity search. Returns envelope with snippets.
    #[tool(description = "Semantic search. Returns an envelope with 200-char snippets, \
                          similarity scores, and honest truncated/total_available. \
                          Call get_memory(id) to read full content.")]
    pub async fn search_memories(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let inner = tools::SearchArgs {
            profile: args.profile,
            query: args.query,
            k: args.k,
            max_tokens: args.max_tokens,
            tags: args.tags,
            min_similarity: args.min_similarity,
        };
        let out = tools::search::handle(&self.pool, self.embedder.clone(), inner)
            .await
            .map_err(chitta_to_rmcp)?;
        let v = serde_json::to_value(&out).map_err(json_to_rmcp)?;
        Ok(Json(v))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ChittaServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "chitta-rs v0.0.1 — agent-native persistent memory. \
                 Three tools: store_memory, get_memory, search_memories. \
                 Profiles isolate namespaces; idempotency_key dedupes writes; \
                 bi-temporal (event_time + record_time); verbatim storage."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ---- Error translation -----------------------------------------------

fn chitta_to_rmcp(e: ChittaError) -> ErrorData {
    let code = e.code();
    let message = e.message();
    let data = serde_json::to_value(e.data()).ok();
    // rmcp exposes named constructors for specific codes; fall back to `new`
    // so our mapping stays authoritative.
    if code == crate::error::codes::INVALID_PARAMS {
        ErrorData::invalid_params(message, data)
    } else {
        ErrorData::internal_error(message, data)
    }
}

fn json_to_rmcp(e: serde_json::Error) -> ErrorData {
    ErrorData::internal_error(
        format!("failed to serialize response: {e}"),
        Some(serde_json::json!({
            "tool": "server",
            "constraint": "response serializes to JSON",
            "next_action": "Report this as a bug; include server logs.",
        })),
    )
}
