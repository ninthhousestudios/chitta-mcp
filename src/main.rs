//! chitta-rs binary entrypoint. See `docs/starting-shape.md` for scope.

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

use chitta_rs::{config::Config, db, embedding::Embedder, mcp::ChittaServer};

/// chitta-rs: agent-native persistent memory MCP server.
#[derive(Debug, Parser)]
#[command(name = "chitta-rs", version, about)]
struct Cli {
    /// Reserved for v0.0.2. Using it exits cleanly with a message.
    #[arg(long, hide = true)]
    http: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Best-effort: .env may not exist, that's fine.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    if cli.http {
        eprintln!(
            "HTTP transport lands in v0.0.2. v0.0.1 is stdio-only — run without --http."
        );
        std::process::exit(2);
    }

    let cfg = Config::from_env().context("loading configuration from environment")?;

    // Logs go to stderr so stdout stays reserved for the MCP frame stream.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        model_path = ?cfg.model_path,
        "starting chitta-rs"
    );

    let pool = db::connect(&cfg).await.context("connecting to database")?;
    db::run_migrations(&pool).await.context("running migrations")?;

    let embedder = Embedder::load(&cfg.model_file(), &cfg.tokenizer_file())
        .context("loading embedding model")?;

    let server = ChittaServer::new(pool, Arc::clone(&embedder));
    let (stdin, stdout) = stdio();

    let service = server
        .serve((stdin, stdout))
        .await
        .context("starting MCP service over stdio")?;

    tokio::select! {
        res = service.waiting() => {
            res.context("MCP service terminated with error")?;
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received; exiting");
        }
    }

    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = int.recv() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    // Non-Unix fallback: Ctrl-C only (no SIGTERM equivalent). v0.0.1 targets
    // Linux/macOS; this keeps the code compiling on Windows for contributors
    // running `cargo check` locally.
    let _ = tokio::signal::ctrl_c().await;
}
