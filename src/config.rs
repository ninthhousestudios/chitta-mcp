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

        Ok(Self { database_url, model_path, log_level })
    }

    pub fn model_file(&self) -> PathBuf {
        self.model_path.join("bge_m3_model.onnx")
    }

    pub fn tokenizer_file(&self) -> PathBuf {
        self.model_path.join("tokenizer.json")
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
