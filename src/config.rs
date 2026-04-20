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

    // Tests mutate process env; run them sequentially to avoid cross-test
    // interference. `cargo test` defaults to parallel, so we gate on a mutex.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn missing_database_url_is_invalid_params() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Safe because the lock serializes env access in this test module.
        unsafe { std::env::remove_var("DATABASE_URL") };
        let err = Config::from_env().unwrap_err();
        match err {
            ChittaError::MissingConfig { name, next_action } => {
                assert_eq!(name, "DATABASE_URL");
                assert!(next_action.contains("DATABASE_URL"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn defaults_when_only_database_url_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("DATABASE_URL", "postgres://localhost/chitta_rs");
            std::env::remove_var("CHITTA_MODEL_PATH");
            std::env::remove_var("CHITTA_LOG_LEVEL");
        }
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.log_level, "info");
        assert!(cfg.model_path.ends_with(".cache/chitta/bge-m3-onnx"));
        assert!(cfg.model_file().ends_with("bge_m3_model.onnx"));
        assert!(cfg.tokenizer_file().ends_with("tokenizer.json"));
    }
}
