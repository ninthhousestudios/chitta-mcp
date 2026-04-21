//! Environment-driven configuration.
//!
//! Loaded once at startup via [`Config::from_env`]. No file formats, no
//! runtime reconfiguration — restart to change settings.

use std::path::PathBuf;

use crate::error::{ChittaError, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub model_path: PathBuf,
    pub log_level: String,
    pub db_max_connections: u32,
    pub db_acquire_timeout_secs: u64,
    pub db_idle_timeout_secs: u64,
    /// Number of independent ONNX sessions in the embedder pool.
    /// Each session loads the full BGE-M3 graph (~1-2 GB RAM). Default 1.
    pub embedder_pool_size: usize,
    /// Whether to log search queries to `query_log` for retrieval research.
    /// Parsed from `CHITTA_QUERY_LOG` env var. Default `true`.
    pub query_log: bool,
    /// HTTP listen address. Parsed from `CHITTA_HTTP_ADDR`. Default `127.0.0.1`.
    pub http_addr: String,
    /// HTTP listen port. Parsed from `CHITTA_HTTP_PORT`. Default `3100`.
    pub http_port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL").map_err(|_| ChittaError::MissingConfig {
            name: "DATABASE_URL",
            next_action:
                "Set DATABASE_URL in the environment or .env file (e.g. postgres://localhost/chitta_rs)."
                    .to_string(),
        })?;

        let model_path = std::env::var_os("CHITTA_MODEL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(default_model_path);

        let log_level = std::env::var("CHITTA_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let db_max_connections: u32 = parse_env_or("CHITTA_DB_MAX_CONNECTIONS", 8);
        let db_acquire_timeout_secs: u64 = parse_env_or("CHITTA_DB_ACQUIRE_TIMEOUT", 5);
        let db_idle_timeout_secs: u64 = parse_env_or("CHITTA_DB_IDLE_TIMEOUT", 600);

        // Each session loads ~1-2 GB RAM (full BGE-M3 graph). Default 1
        // preserves v0.0.1 memory footprint; increase only when concurrent
        // embedding throughput justifies the RAM cost.
        let embedder_pool_size: usize = parse_env_or("CHITTA_EMBEDDER_POOL_SIZE", 1)
            .max(1); // floor at 1 — zero sessions is nonsensical

        // Default true; only "false" (case-insensitive) disables.
        let query_log: bool = std::env::var("CHITTA_QUERY_LOG")
            .map(|v| !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);

        let http_addr =
            std::env::var("CHITTA_HTTP_ADDR").unwrap_or_else(|_| "127.0.0.1".to_string());

        let http_port: u16 = parse_env_or("CHITTA_HTTP_PORT", 3100);

        Ok(Self {
            database_url,
            model_path,
            log_level,
            db_max_connections,
            db_acquire_timeout_secs,
            db_idle_timeout_secs,
            embedder_pool_size,
            query_log,
            http_addr,
            http_port,
        })
    }

    pub fn model_file(&self) -> PathBuf {
        self.model_path.join("bge_m3_model.onnx")
    }

    pub fn tokenizer_file(&self) -> PathBuf {
        self.model_path.join("tokenizer.json")
    }
}

fn parse_env_or<T: std::str::FromStr + std::fmt::Display>(name: &str, default: T) -> T {
    match std::env::var(name) {
        Err(_) => default,
        Ok(v) => match v.parse() {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!(
                    "WARNING: {name}={v:?} is not a valid {ty} — using default {default}",
                    ty = std::any::type_name::<T>(),
                );
                default
            }
        },
    }
}

fn default_model_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".cache/chitta/bge-m3-onnx");
        p
    } else {
        PathBuf::from(".cache/chitta/bge-m3-onnx")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests in this module mutate the process-wide environment. Rust 2024
    // marks `std::env::set_var`/`remove_var` `unsafe` because concurrent env
    // access from other threads is UB. We confine every env mutation to
    // `with_env` below, which:
    //   1. Acquires a module-static Mutex so only one test holds env at a time.
    //   2. Applies the requested deltas, runs the closure, then restores the
    //      prior values regardless of panic outcome.
    // Keeping `unsafe` blocks in exactly one place makes the invariant
    // auditable: every env write in this crate's test code goes through here.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Apply `deltas` (name, value — `None` removes the var) around `f`, then
    /// restore prior values. Serialized via [`ENV_LOCK`]; safe as long as no
    /// other code in the crate mutates env outside this helper.
    fn with_env<R>(deltas: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prior: Vec<(String, Option<String>)> = deltas
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();
        // Safe: ENV_LOCK serializes every env-touching test in this crate.
        unsafe {
            for (k, v) in deltas {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        unsafe {
            for (k, v) in &prior {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        match result {
            Ok(r) => r,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    #[test]
    fn missing_database_url_is_invalid_params() {
        with_env(&[("DATABASE_URL", None)], || {
            let err = Config::from_env().unwrap_err();
            match err {
                ChittaError::MissingConfig { name, next_action } => {
                    assert_eq!(name, "DATABASE_URL");
                    assert!(next_action.contains("DATABASE_URL"));
                }
                other => panic!("unexpected error: {other:?}"),
            }
        });
    }

    #[test]
    fn defaults_when_only_database_url_set() {
        with_env(
            &[
                ("DATABASE_URL", Some("postgres://localhost/chitta_rs")),
                ("CHITTA_MODEL_PATH", None),
                ("CHITTA_LOG_LEVEL", None),
            ],
            || {
                let cfg = Config::from_env().unwrap();
                assert_eq!(cfg.log_level, "info");
                assert!(cfg.model_path.ends_with(".cache/chitta/bge-m3-onnx"));
                assert!(cfg.model_file().ends_with("bge_m3_model.onnx"));
                assert!(cfg.tokenizer_file().ends_with("tokenizer.json"));
            },
        );
    }
}
