//! Argument validation. One rule per fn; each returns an
//! [`InvalidArgument`](crate::error::ChittaError::InvalidArgument) with a
//! populated `constraint` + `next_action` on failure (Principle 8).

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::error::{ChittaError, Result};

/// Profile: 1-128 chars, `[a-zA-Z0-9_-]+`.
pub fn profile(tool: &'static str, value: &str) -> Result<()> {
    let len_ok = !value.is_empty() && value.len() <= 128;
    let chars_ok =
        value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !len_ok || !chars_ok {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "profile".to_string(),
            constraint: "1-128 chars, [a-zA-Z0-9_-]+ only".to_string(),
            received: Some(json!(value)),
            next_action:
                "Pass a non-empty profile of ≤128 ASCII letters, digits, underscores, or hyphens."
                    .to_string(),
        });
    }
    Ok(())
}

/// Content: non-empty. Token-length bound is enforced inside `embed`.
pub fn content_non_empty(tool: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "content".to_string(),
            constraint: "length >= 1".to_string(),
            received: Some(json!("")),
            next_action: "Pass non-empty content.".to_string(),
        });
    }
    Ok(())
}

/// Idempotency key: 1-128 chars, no control characters.
pub fn idempotency_key(tool: &'static str, value: &str) -> Result<()> {
    let len_ok = !value.is_empty() && value.len() <= 128;
    let no_control = value.chars().all(|c| !c.is_control());
    if !len_ok || !no_control {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "idempotency_key".to_string(),
            constraint: "1-128 chars, no control characters".to_string(),
            received: Some(json!(value)),
            next_action:
                "Pass a 1-128 character idempotency_key with no control characters (e.g. a UUID \
                 or a client-stable hash)."
                    .to_string(),
        });
    }
    Ok(())
}

/// Event time: `>= 1970-01-01T00:00:00Z` and `<= now + 365 days`.
pub fn event_time(tool: &'static str, value: DateTime<Utc>) -> Result<()> {
    let epoch = Utc.timestamp_opt(0, 0).single().expect("epoch is valid");
    let upper = Utc::now() + Duration::days(365);
    if value < epoch {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "event_time".to_string(),
            constraint: "ISO-8601 timestamp >= 1970-01-01T00:00:00Z".to_string(),
            received: Some(json!(value.to_rfc3339())),
            next_action:
                "Pass event_time >= 1970-01-01T00:00:00Z, or omit to default to record_time."
                    .to_string(),
        });
    }
    if value > upper {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "event_time".to_string(),
            constraint: "ISO-8601 timestamp <= now + 365 days".to_string(),
            received: Some(json!(value.to_rfc3339())),
            next_action:
                "Pass event_time within one year of now, or omit to default to record_time."
                    .to_string(),
        });
    }
    Ok(())
}

/// Tags: up to 32 entries, each 1-64 chars.
pub fn tags(tool: &'static str, values: &[String]) -> Result<()> {
    if values.len() > 32 {
        return Err(ChittaError::InvalidArgument {
            tool,
            argument: "tags".to_string(),
            constraint: "at most 32 tags".to_string(),
            received: Some(json!({ "count": values.len() })),
            next_action: "Trim the tag list to at most 32 entries.".to_string(),
        });
    }
    for (i, t) in values.iter().enumerate() {
        if t.is_empty() || t.len() > 64 {
            return Err(ChittaError::InvalidArgument {
                tool,
                argument: "tags".to_string(),
                constraint: "each tag 1-64 chars".to_string(),
                received: Some(json!({ "index": i, "length": t.len() })),
                next_action: "Ensure every tag is between 1 and 64 characters.".to_string(),
            });
        }
    }
    Ok(())
}

/// Parse a UUID argument, translating parse errors to a populated
/// `InvalidArgument`.
pub fn parse_uuid(tool: &'static str, argument: &'static str, value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|e| ChittaError::InvalidArgument {
        tool,
        argument: argument.to_string(),
        constraint: "valid UUID".to_string(),
        received: Some(json!(value)),
        next_action: format!("Pass a valid UUID string. Parse error: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_rules() {
        assert!(profile("t", "default").is_ok());
        assert!(profile("t", "alpha_beta-1").is_ok());
        assert!(profile("t", "").is_err());
        assert!(profile("t", "has space").is_err());
        assert!(profile("t", &"a".repeat(129)).is_err());
    }

    #[test]
    fn idempotency_key_rules() {
        assert!(idempotency_key("t", "key-1").is_ok());
        assert!(idempotency_key("t", "").is_err());
        assert!(idempotency_key("t", "has\ncontrol").is_err());
    }

    #[test]
    fn event_time_rules() {
        let pre_epoch = Utc.with_ymd_and_hms(1969, 6, 20, 0, 0, 0).single().unwrap();
        assert!(event_time("t", pre_epoch).is_err());
        let now = Utc::now();
        assert!(event_time("t", now).is_ok());
        let too_far = now + Duration::days(400);
        assert!(event_time("t", too_far).is_err());
    }

    #[test]
    fn tag_rules() {
        assert!(tags("t", &[]).is_ok());
        assert!(tags("t", &["a".to_string(), "b".to_string()]).is_ok());
        let too_many: Vec<String> = (0..33).map(|i| format!("t{i}")).collect();
        assert!(tags("t", &too_many).is_err());
        assert!(tags("t", &["".to_string()]).is_err());
        assert!(tags("t", &["x".repeat(65)]).is_err());
    }
}
