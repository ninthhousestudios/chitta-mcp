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

// ---- Tool handlers ---------------------------------------------------
//
// The `tools::*Args` types carry `#[derive(JsonSchema)]` directly, so rmcp
// publishes them on the wire from a single source of truth — no mirror
// structs, no field-drift risk.

#[tool_router(router = tool_router)]
impl ChittaServer {
    /// Store a new memory. Idempotent on (profile, idempotency_key).
    #[tool(description = "Store a new memory. Idempotent on (profile, idempotency_key): \
                          resubmitting the same key returns the prior row with \
                          idempotent_replay=true.")]
    pub async fn store_memory(
        &self,
        Parameters(args): Parameters<tools::StoreArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let out = tools::store::handle(&self.pool, self.embedder.clone(), args)
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
        Parameters(args): Parameters<tools::GetArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let out = tools::get::handle(&self.pool, args).await.map_err(chitta_to_rmcp)?;
        let v = serde_json::to_value(&out).map_err(json_to_rmcp)?;
        Ok(Json(v))
    }

    /// Semantic similarity search. Returns envelope with snippets.
    #[tool(description = "Semantic search. Returns an envelope with 200-char snippets, \
                          similarity scores, and honest truncated/total_available. \
                          Call get_memory(id) to read full content.")]
    pub async fn search_memories(
        &self,
        Parameters(args): Parameters<tools::SearchArgs>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let out = tools::search::handle(&self.pool, self.embedder.clone(), args)
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

/// Translate a [`ChittaError`] into rmcp's `ErrorData` shape. `pub` so the
/// contract test suite can exercise every variant through the actual mapper
/// — if this fn ever drops a field or misroutes a code, `tests/contract.rs`
/// catches it.
pub fn chitta_to_rmcp(e: ChittaError) -> ErrorData {
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
