//! BGE-M3 dense embeddings via ONNX Runtime.
//!
//! Contract: one [`Embedder`] per process, shared via `Arc`. Each `embed`
//! call tokenizes, rejects > 8192 tokens, runs the ONNX session, and returns
//! a 1024-dim `Vec<f32>`.
//!
//! # Concurrency caveat (read before enabling new transports)
//!
//! The ONNX session lives behind a blocking [`Mutex`]. `ort 2.0.0-rc.10`'s
//! `Session::run` takes `&mut self`, so we can't share it directly. For
//! v0.0.1 this is fine because the stdio transport is single-request by
//! construction: only one tool call is ever in flight. **The moment a
//! transport pipelines requests — v0.0.2 HTTP, any concurrent MCP client
//! — this mutex becomes a global embedding bottleneck.** Before shipping
//! such a transport, replace the mutex with (a) a dedicated embedder
//! thread pool fed by an mpsc queue, or (b) an `ort` version that supports
//! `&self` inference, or (c) a pool of sessions. Do not add a second
//! transport without revisiting this.
//!
//! # Model output note
//!
//! The `yuniko-software/bge-m3-onnx` export (which the Python chitta reads
//! at `~/.cache/chitta/bge-m3-onnx/bge_m3_model.onnx`) exposes a named output
//! `dense_embeddings` of shape `[batch, 1024]`. Pooling (CLS token) and L2
//! normalization are performed *inside* the exported graph, so the host does
//! no post-processing. The plan doc describes a `[1, seq_len, 1024]` output
//! that the host would pool and normalize — that shape applies to a different
//! BGE-M3 export. We follow the actual behavior of the file on disk.
//!
//! The `sparse_weights` output is ignored in v0.0.1 (no sparse column in the
//! starting shape).

use std::path::Path;
use std::sync::{Arc, Mutex};

use ndarray::Array2;
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Value;
use tokenizers::Tokenizer;

use crate::error::{ChittaError, Result};

/// Dimension of BGE-M3's dense output. Pinned here so a dimension drift
/// shows up as a loud panic in tests rather than a silent write.
pub const EMBEDDING_DIM: usize = 1024;

/// Hard cap enforced before we run the session (Principle 1: we never
/// embed a truncated version of stored content).
pub const MAX_TOKENS: usize = 8192;

pub struct Embedder {
    tokenizer: Tokenizer,
    // `Session::run` takes `&mut self` in ort 2.0.0-rc.10, so we guard it with
    // a blocking mutex. Embedding is CPU-bound and already serialized in v0.0.1
    // (stdio handles one request at a time); the mutex is not on any hot path.
    session: Mutex<Session>,
}

impl Embedder {
    pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Arc<Self>> {
        tracing::info!(model = ?model_path, tokenizer = ?tokenizer_path, "loading BGE-M3");

        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| ChittaError::Embedding {
                tool: "startup",
                message: format!("failed to load tokenizer at {tokenizer_path:?}: {e}"),
                next_action:
                    "Ensure CHITTA_MODEL_PATH contains a valid HuggingFace tokenizer.json."
                        .to_string(),
            })?;

        let session = Session::builder()
            .map_err(embedding_startup_err)?
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .map_err(embedding_startup_err)?
            .commit_from_file(model_path)
            .map_err(|e| ChittaError::Embedding {
                tool: "startup",
                message: format!("failed to load ONNX model at {model_path:?}: {e}"),
                next_action:
                    "Ensure CHITTA_MODEL_PATH contains bge_m3_model.onnx (and its .onnx_data \
                     sidecar) and that libonnxruntime is installed or ORT_DYLIB_PATH is set."
                        .to_string(),
            })?;

        Ok(Arc::new(Self { tokenizer, session: Mutex::new(session) }))
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| ChittaError::Embedding {
                tool: "store_memory",
                message: format!("tokenizer error: {e}"),
                next_action:
                    "Tokenizer failed to encode the input. Verify CHITTA_MODEL_PATH contains the \
                     tokenizer.json that matches the deployed BGE-M3 ONNX export."
                        .to_string(),
            })?;

        let ids = encoding.get_ids();
        let attn = encoding.get_attention_mask();

        if ids.len() > MAX_TOKENS {
            return Err(ChittaError::ContentTooLong {
                tool: "store_memory",
                token_count: ids.len(),
            });
        }

        let seq_len = ids.len();
        let input_ids: Vec<i64> = ids.iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = attn.iter().map(|&m| m as i64).collect();

        let input_ids_arr = Array2::from_shape_vec((1, seq_len), input_ids).map_err(|e| {
            ChittaError::Internal(format!("failed to build input_ids tensor: {e}"))
        })?;
        let attention_mask_arr =
            Array2::from_shape_vec((1, seq_len), attention_mask).map_err(|e| {
                ChittaError::Internal(format!("failed to build attention_mask tensor: {e}"))
            })?;

        let input_ids_value = Value::from_array(input_ids_arr).map_err(ort_to_embed_err)?;
        let attention_mask_value =
            Value::from_array(attention_mask_arr).map_err(ort_to_embed_err)?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| ChittaError::Internal(format!("embedding session mutex poisoned: {e}")))?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids_value,
                "attention_mask" => attention_mask_value,
            ])
            .map_err(ort_to_embed_err)?;

        let dense = outputs
            .get("dense_embeddings")
            .ok_or_else(|| ChittaError::Embedding {
                tool: "store_memory",
                message: "ONNX session produced no `dense_embeddings` output".to_string(),
                next_action: "Report this as a bug; include server logs.".to_string(),
            })?;

        let (shape, data) = dense.try_extract_tensor::<f32>().map_err(ort_to_embed_err)?;

        // Expected shape is [1, 1024]; accept either [1, 1024] or [1024]
        // to stay robust against minor export differences.
        let total: usize = shape.iter().map(|&d| d as usize).product();
        if total != EMBEDDING_DIM {
            return Err(ChittaError::Embedding {
                tool: "store_memory",
                message: format!(
                    "unexpected embedding shape {shape:?}; expected {EMBEDDING_DIM} elements"
                ),
                next_action: "Report this as a bug; include server logs.".to_string(),
            });
        }

        Ok(data.to_vec())
    }
}

fn embedding_startup_err(e: ort::Error) -> ChittaError {
    ChittaError::Embedding {
        tool: "startup",
        message: format!("ONNX session builder failed: {e}"),
        next_action:
            "Ensure libonnxruntime is installed (or ORT_DYLIB_PATH is set) and the model files \
             are present."
                .to_string(),
    }
}

fn ort_to_embed_err(e: ort::Error) -> ChittaError {
    ChittaError::Embedding {
        tool: "store_memory",
        message: format!("ONNX runtime error: {e}"),
        next_action: "Report this as a bug; include server logs.".to_string(),
    }
}
