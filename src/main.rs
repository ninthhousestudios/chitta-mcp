//! chitta-rs binary entrypoint. See `docs/starting-shape.md` for scope.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use chitta_rs::{config::Config, db, embedding::Embedder, mcp::ChittaServer};

/// chitta-rs: agent-native persistent memory MCP server.
#[derive(Debug, Parser)]
#[command(name = "chitta-rs", version, about)]
struct Cli {
    /// Reserved for v0.0.2. Using it exits cleanly with a message.
    #[arg(long, hide = true)]
    http: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run chitta-rs as a stdio MCP server (default when no subcommand given).
    Serve,
    /// Re-run logged queries against current DB state for regression detection.
    Replay {
        /// Filter to a specific memory profile.
        #[arg(long)]
        profile: Option<String>,
        /// Maximum number of query_log entries to replay.
        #[arg(long, default_value = "100")]
        limit: i64,
    },
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

    match cli.command {
        Some(Commands::Replay { profile, limit }) => run_replay(profile, limit).await,
        // `serve` subcommand or no subcommand — both run the MCP server.
        Some(Commands::Serve) | None => run_serve().await,
    }
}

async fn run_serve() -> Result<()> {
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

    let query_log_enabled = cfg.query_log
        && sqlx::query("SELECT 1 FROM query_log LIMIT 0")
            .execute(&pool)
            .await
            .map_err(|e| {
                tracing::warn!("query_log table missing — search logging disabled: {e}");
                e
            })
            .is_ok();
    if query_log_enabled {
        tracing::info!("query_log enabled");
    }

    let embedder = Embedder::load(
        &cfg.model_file(),
        &cfg.tokenizer_file(),
        cfg.embedder_pool_size,
    )
    .context("loading embedding model")?;

    let server = ChittaServer::new(pool, Arc::clone(&embedder), query_log_enabled);
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

async fn run_replay(profile: Option<String>, limit: i64) -> Result<()> {
    let cfg = Config::from_env().context("loading configuration from environment")?;

    // Replay writes to stdout — keep tracing on stderr, but silence it by
    // default so the table isn't polluted with log lines.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("warn"))
        .with_writer(std::io::stderr)
        .init();

    let pool = db::connect(&cfg).await.context("connecting to database")?;
    // Migrations may not have run yet in some envs; run them to be safe.
    db::run_migrations(&pool).await.context("running migrations")?;

    let entries = db::read_query_log(&pool, profile.as_deref(), limit)
        .await
        .context("reading query_log")?;

    if entries.is_empty() {
        println!("No query_log entries found.");
        return Ok(());
    }

    println!("Replay Results ({} queries):", entries.len());
    println!("┌─────┬──────────┬──────────────────────────────────────────────────────┬─────────┬──────────┬─────────┐");
    println!("│ {:<3} │ {:<8} │ {:<52} │ {:<7} │ {:<8} │ {:<7} │", "#", "Profile", "Query (50ch)", "Overlap", "New", "Dropped");
    println!("├─────┼──────────┼──────────────────────────────────────────────────────┼─────────┼──────────┼─────────┤");

    let mut total_overlap: f64 = 0.0;

    for (idx, entry) in entries.iter().enumerate() {
        let (new_hits, _total) = db::search_by_embedding(
            &pool,
            &entry.profile,
            &entry.embedding,
            entry.k as i64,
            &entry.tags,
            entry.min_similarity,
        )
        .await
        .context("re-running search")?;

        let logged_ids: HashSet<Uuid> = entry.result_ids.iter().copied().collect();
        let new_ids: HashSet<Uuid> = new_hits.iter().map(|h| h.id).collect();

        let intersection = logged_ids.intersection(&new_ids).count();
        let union = logged_ids.union(&new_ids).count();

        let overlap_pct = if union == 0 {
            100.0_f64
        } else {
            intersection as f64 / union as f64 * 100.0
        };

        let new_count = new_ids.difference(&logged_ids).count();
        let dropped_count = logged_ids.difference(&new_ids).count();

        total_overlap += overlap_pct;

        // Truncate query text to 50 chars for display.
        let query_display: String = entry.query_text.chars().take(50).collect();
        let query_display = if entry.query_text.chars().count() > 50 {
            format!("{query_display}...")
        } else {
            query_display
        };

        // Truncate profile to 8 chars.
        let profile_display: String = entry.profile.chars().take(8).collect();

        println!(
            "│ {:<3} │ {:<8} │ {:<52} │ {:>6.0}% │ {:<8} │ {:<7} │",
            idx + 1,
            profile_display,
            query_display,
            overlap_pct,
            new_count,
            dropped_count,
        );
    }

    println!("└─────┴──────────┴──────────────────────────────────────────────────────┴─────────┴──────────┴─────────┘");

    let avg_overlap = total_overlap / entries.len() as f64;
    println!();
    println!(
        "Summary: {:.0}% average overlap across {} queries",
        avg_overlap,
        entries.len()
    );

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
