//! chitta-rs binary entrypoint. See `docs/starting-shape.md` for scope.

use std::collections::HashSet;
use std::path::PathBuf;
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
    /// Run as a Streamable HTTP server instead of stdio.
    #[arg(long)]
    http: bool,

    /// HTTP listen address (used with --http).
    #[arg(long, default_value = "127.0.0.1")]
    http_addr: String,

    /// HTTP listen port (used with --http).
    #[arg(long, default_value = "3100")]
    http_port: u16,

    /// Path to a file containing the bearer token for HTTP auth (required with --http).
    #[arg(long)]
    auth_token_file: Option<PathBuf>,

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
    match cli.command {
        Some(Commands::Replay { profile, limit }) => return run_replay(profile, limit).await,
        Some(Commands::Serve) | None => {}
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

    if cli.http {
        serve_http(cli, cfg, pool, embedder, query_log_enabled).await
    } else {
        serve_stdio(pool, embedder, query_log_enabled).await
    }
}

/// Stdio transport — the original v0.0.1 path.
async fn serve_stdio(
    pool: sqlx::PgPool,
    embedder: Arc<Embedder>,
    query_log_enabled: bool,
) -> Result<()> {
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

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("warn"))
        .with_writer(std::io::stderr)
        .init();

    let pool = db::connect(&cfg).await.context("connecting to database")?;
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

        let query_display: String = entry.query_text.chars().take(50).collect();
        let query_display = if entry.query_text.chars().count() > 50 {
            format!("{query_display}...")
        } else {
            query_display
        };

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

/// Streamable HTTP transport with bearer-token auth.
async fn serve_http(
    cli: Cli,
    cfg: Config,
    pool: sqlx::PgPool,
    embedder: Arc<Embedder>,
    query_log_enabled: bool,
) -> Result<()> {
    use axum::routing::any_service;
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager,
        tower::{StreamableHttpServerConfig, StreamableHttpService},
    };
    use tokio_util::sync::CancellationToken;
    use tower_http::validate_request::ValidateRequestHeaderLayer;

    let token_path = cli.auth_token_file.ok_or_else(|| {
        anyhow::anyhow!("--auth-token-file is required when running in --http mode")
    })?;
    let bearer_token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading auth token from {}", token_path.display()))?
        .trim()
        .to_string();
    anyhow::ensure!(!bearer_token.is_empty(), "auth token file is empty");

    let cancel = CancellationToken::new();

    let http_addr = if cli.http_addr != "127.0.0.1" {
        cli.http_addr.clone()
    } else {
        cfg.http_addr.clone()
    };
    let http_port = if cli.http_port != 3100 { cli.http_port } else { cfg.http_port };

    let mut allowed_hosts = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if http_addr != "127.0.0.1" && http_addr != "localhost" && http_addr != "::1" {
        allowed_hosts.push(http_addr.clone());
    }

    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancel.clone())
        .with_allowed_hosts(allowed_hosts);

    let session_manager = Arc::new(LocalSessionManager::default());

    let pool_clone = pool.clone();
    let embedder_clone = Arc::clone(&embedder);
    let ql = query_log_enabled;
    let mcp_service = StreamableHttpService::new(
        move || Ok(ChittaServer::new(pool_clone.clone(), Arc::clone(&embedder_clone), ql)),
        session_manager,
        config,
    );

    #[allow(deprecated)]
    let app = axum::Router::new()
        .route("/mcp", any_service(mcp_service))
        .layer(ValidateRequestHeaderLayer::bearer(&bearer_token));

    let addr = format!("{http_addr}:{http_port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e} — is the port in use?"))?;
    tracing::info!(%addr, "chitta-rs HTTP server listening");

    let cancel_for_shutdown = cancel.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutdown signal received, draining connections");
            cancel_for_shutdown.cancel();
        })
        .await
        .context("HTTP server exited with error")?;


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
