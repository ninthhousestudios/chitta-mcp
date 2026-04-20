//! L0 contract tests: pure schema / serde shape — no DB, no subprocess.
//!
//! These tests lock in the wire contract described in
//! `rust/docs/starting-shape.md`. If a field is renamed, removed, or its type
//! changes, a test here fails loudly before integration tests or a caller
//! even notice.

use chitta_rs::envelope::Envelope;
use chitta_rs::error::{ChittaError, codes};
use chitta_rs::tools::{GetArgs, GetOutput, SearchArgs, SearchHit, SearchOutput, StoreArgs, StoreOutput};
use serde_json::{Value, json};

/// Helper: assert that `value` is a JSON object and every `key` is present.
fn assert_keys(value: &Value, keys: &[&str]) {
    let obj = value.as_object().expect("object");
    for k in keys {
        assert!(obj.contains_key(*k), "missing key `{k}` in {value}");
    }
}

// ---- Arguments (wire -> struct) -------------------------------------

#[test]
fn store_args_accepts_minimum_fields() {
    let v = json!({
        "profile": "p",
        "content": "hello",
        "idempotency_key": "k",
    });
    let args: StoreArgs = serde_json::from_value(v).unwrap();
    assert_eq!(args.profile, "p");
    assert_eq!(args.content, "hello");
    assert_eq!(args.idempotency_key, "k");
    assert!(args.event_time.is_none());
    assert!(args.tags.is_none());
}

#[test]
fn store_args_accepts_full_payload() {
    let v = json!({
        "profile": "p",
        "content": "hello",
        "idempotency_key": "k",
        "event_time": "2026-01-02T03:04:05Z",
        "tags": ["alpha", "beta"],
    });
    let args: StoreArgs = serde_json::from_value(v).unwrap();
    assert!(args.event_time.is_some());
    assert_eq!(args.tags.unwrap(), vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn get_args_shape() {
    let v = json!({"profile": "p", "id": "7e…"});
    let args: GetArgs = serde_json::from_value(v).unwrap();
    assert_eq!(args.profile, "p");
    assert_eq!(args.id, "7e…");
}

#[test]
fn search_args_all_optional_except_required() {
    let v = json!({"profile": "p", "query": "q"});
    let args: SearchArgs = serde_json::from_value(v).unwrap();
    assert!(args.k.is_none());
    assert!(args.max_tokens.is_none());
    assert!(args.tags.is_none());
    assert!(args.min_similarity.is_none());
}

// ---- Outputs (struct -> wire) ---------------------------------------

#[test]
fn store_output_wire_keys() {
    let t = chrono::Utc::now();
    let out = StoreOutput {
        id: uuid::Uuid::now_v7(),
        profile: "p".into(),
        content: "c".into(),
        event_time: t,
        record_time: t,
        tags: vec![],
        idempotent_replay: false,
    };
    let v = serde_json::to_value(&out).unwrap();
    assert_keys(
        &v,
        &["id", "profile", "content", "event_time", "record_time", "tags", "idempotent_replay"],
    );
    assert_eq!(v["idempotent_replay"], json!(false));
}

#[test]
fn get_output_wire_keys() {
    let t = chrono::Utc::now();
    let out = GetOutput {
        id: uuid::Uuid::now_v7(),
        profile: "p".into(),
        content: "c".into(),
        event_time: t,
        record_time: t,
        tags: vec!["x".into()],
    };
    let v = serde_json::to_value(&out).unwrap();
    assert_keys(&v, &["id", "profile", "content", "event_time", "record_time", "tags"]);
}

#[test]
fn search_output_envelope_shape() {
    let t = chrono::Utc::now();
    let hit = SearchHit {
        id: uuid::Uuid::now_v7(),
        snippet: "snip".into(),
        similarity: 0.88,
        event_time: t,
        record_time: t,
        tags: vec![],
    };
    let env: SearchOutput = Envelope::new(vec![hit], false, Some(1), 42);
    let v = serde_json::to_value(&env).unwrap();
    assert_keys(&v, &["results", "truncated", "total_available", "budget_spent_tokens"]);
    let first = &v["results"][0];
    assert_keys(first, &["id", "snippet", "similarity", "event_time", "record_time", "tags"]);
}

// ---- Error contract ------------------------------------------------

/// Every error must carry `tool`, `constraint`, `next_action` on the wire.
/// This is Principle 8's enforcement from the caller's perspective — it
/// matches `error::tests::every_variant_populates_contract`, but serializes
/// through `serde_json::to_value` to catch any accidental skip-serialize
/// attribute that would hide a field from the wire.
#[test]
fn every_error_variant_serializes_with_contract_fields() {
    use std::io;

    let variants = vec![
        ChittaError::MissingConfig {
            name: "DATABASE_URL",
            next_action: "set it".to_string(),
        },
        ChittaError::InvalidArgument {
            tool: "store_memory",
            argument: "profile".to_string(),
            constraint: "1-128 chars".to_string(),
            received: Some(json!("")),
            next_action: "pass a profile".to_string(),
        },
        ChittaError::ContentTooLong { tool: "store_memory", token_count: 9001 },
        ChittaError::NotFound {
            tool: "get_memory",
            kind: "memory",
            next_action: "verify id".to_string(),
        },
        ChittaError::Embedding {
            tool: "store_memory",
            message: "ort error".to_string(),
            next_action: "restart".to_string(),
        },
        ChittaError::Db(sqlx::Error::PoolTimedOut),
        ChittaError::Db(sqlx::Error::Io(io::Error::other("connection reset"))),
        ChittaError::Migrate(sqlx::migrate::MigrateError::Execute(
            sqlx::Error::Io(io::Error::other("drift")),
        )),
        ChittaError::Internal("unexpected".to_string()),
    ];

    for e in &variants {
        let data = serde_json::to_value(e.data()).unwrap();
        let obj = data.as_object().expect("object");

        let tool = obj.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let constraint = obj.get("constraint").and_then(|v| v.as_str()).unwrap_or("");
        let next_action = obj.get("next_action").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!tool.is_empty(), "empty `tool` for {e:?}");
        assert!(!constraint.is_empty(), "empty `constraint` for {e:?}");
        assert!(!next_action.is_empty(), "empty `next_action` for {e:?}");

        let code = e.code();
        assert!(
            code == codes::INVALID_PARAMS || code == codes::INTERNAL_ERROR,
            "unexpected code {code} for {e:?}"
        );
    }
}

#[test]
fn error_data_skip_serializes_none_fields() {
    let e = ChittaError::NotFound {
        tool: "get_memory",
        kind: "memory",
        next_action: "verify id".to_string(),
    };
    let v = serde_json::to_value(e.data()).unwrap();
    // `argument` and `received` are None for NotFound; they should be
    // absent from the wire payload (not serialized as null).
    assert!(!v.as_object().unwrap().contains_key("argument"));
    assert!(!v.as_object().unwrap().contains_key("received"));
}

// ---- JSON-RPC wire mapping (chitta_to_rmcp) -------------------------
//
// Walk every variant through the mcp-side mapper and assert that the
// resulting `ErrorData` serializes with the JSON-RPC code we expect and a
// `data` payload carrying the Principle 8 triple (`tool`, `constraint`,
// `next_action`). If the mapper drops a field or misroutes a code, this
// test — not a client in the wild — surfaces the regression.

#[test]
fn chitta_to_rmcp_preserves_code_and_contract_fields() {
    use chitta_rs::mcp::chitta_to_rmcp;
    use std::io;

    let variants: Vec<(ChittaError, i32)> = vec![
        (
            ChittaError::MissingConfig {
                name: "DATABASE_URL",
                next_action: "set it".to_string(),
            },
            codes::INVALID_PARAMS,
        ),
        (
            ChittaError::InvalidArgument {
                tool: "store_memory",
                argument: "profile".to_string(),
                constraint: "1-128 chars".to_string(),
                received: Some(json!("")),
                next_action: "pass a profile".to_string(),
            },
            codes::INVALID_PARAMS,
        ),
        (
            ChittaError::ContentTooLong { tool: "store_memory", token_count: 9001 },
            codes::INVALID_PARAMS,
        ),
        (
            ChittaError::NotFound {
                tool: "get_memory",
                kind: "memory",
                next_action: "verify id".to_string(),
            },
            codes::INVALID_PARAMS,
        ),
        (
            ChittaError::Embedding {
                tool: "store_memory",
                message: "ort error".to_string(),
                next_action: "restart".to_string(),
            },
            codes::INTERNAL_ERROR,
        ),
        (ChittaError::Db(sqlx::Error::PoolTimedOut), codes::INTERNAL_ERROR),
        (
            ChittaError::Db(sqlx::Error::Io(io::Error::other("reset"))),
            codes::INTERNAL_ERROR,
        ),
        (
            ChittaError::Migrate(sqlx::migrate::MigrateError::Execute(
                sqlx::Error::Io(io::Error::other("drift")),
            )),
            codes::INTERNAL_ERROR,
        ),
        (ChittaError::Internal("unexpected".to_string()), codes::INTERNAL_ERROR),
    ];

    for (variant, expected_code) in variants {
        // Format the variant for diagnostics before moving it into the mapper.
        let label = format!("{variant:?}");
        let mapped = chitta_to_rmcp(variant);
        let wire = serde_json::to_value(&mapped).expect("ErrorData serializes");
        let obj = wire.as_object().expect("ErrorData is a JSON object");

        let code = obj
            .get("code")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| panic!("missing `code` for {label}: {wire}"));
        assert_eq!(code as i32, expected_code, "code mismatch for {label}");

        let message = obj.get("message").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!message.is_empty(), "empty `message` for {label}");

        let data = obj
            .get("data")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("missing `data` object for {label}: {wire}"));
        for required in ["tool", "constraint", "next_action"] {
            let v = data.get(required).and_then(|v| v.as_str()).unwrap_or("");
            assert!(!v.is_empty(), "missing `data.{required}` for {label}: {wire}");
        }
    }
}
