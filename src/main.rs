//! chitta-rs binary entrypoint. See `docs/starting-shape.md` for scope.

use std::path::PathBuf;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    // Best-effort: .env may not exist, that's fine.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
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

    // 1. Validate auth config — token file is mandatory for HTTP mode.
    let token_path = cli.auth_token_file.ok_or_else(|| {
        anyhow::anyhow!("--auth-token-file is required when running in --http mode")
    })?;
    let bearer_token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("reading auth token from {}", token_path.display()))?
        .trim()
        .to_string();
    anyhow::ensure!(!bearer_token.is_empty(), "auth token file is empty");

    // 2. Build the cancellation token for graceful shutdown.
    let cancel = CancellationToken::new();

    // 3. Resolve listen address (CLI flags override env/config defaults).
    let http_addr = if cli.http_addr != "127.0.0.1" {
        cli.http_addr.clone()
    } else {
        cfg.http_addr.clone()
    };
    let http_port = if cli.http_port != 3100 { cli.http_port } else { cfg.http_port };

    // 4. Build allowed hosts for DNS-rebinding protection.
    let mut allowed_hosts = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if http_addr != "127.0.0.1" && http_addr != "localhost" && http_addr != "::1" {
        allowed_hosts.push(http_addr.clone());
    }

    // 5. Configure the StreamableHTTP server.
    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancel.clone())
        .with_allowed_hosts(allowed_hosts);

    let session_manager = Arc::new(LocalSessionManager::default());

    // 6. Service factory: creates a fresh ChittaServer per session.
    let pool_clone = pool.clone();
    let embedder_clone = Arc::clone(&embedder);
    let ql = query_log_enabled;
    let mcp_service = StreamableHttpService::new(
        move || Ok(ChittaServer::new(pool_clone.clone(), Arc::clone(&embedder_clone), ql)),
        session_manager,
        config,
    );

    // 7. Mount as axum route with bearer-token auth.
    #[allow(deprecated)] // tower-http deprecated the convenience; fine for our use-case
    let app = axum::Router::new()
        .route("/mcp", any_service(mcp_service))
        .layer(ValidateRequestHeaderLayer::bearer(&bearer_token));

    // 8. Bind and serve.
    let addr = format!("{http_addr}:{http_port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e} — is the port in use?"))?;
    tracing::info!(%addr, "chitta-rs HTTP server listening");

    // 9. Graceful shutdown: drain in-flight requests, then cancel sessions.
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
