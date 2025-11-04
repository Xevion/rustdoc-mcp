use cargo_doc_mcp::cargo::UptimeTimer;
use cargo_doc_mcp::cli::Cli;
use cargo_doc_mcp::handlers::legacy;
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() {
    let default_level = if cfg!(debug_assertions) {
        "cargo_doc_mcp=trace,warn"
    } else {
        "cargo_doc_mcp=info,warn"
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::registry()
        .with(fmt::layer().with_timer(UptimeTimer::new()))
        .with(filter)
        .init();

    let cli = Cli::parse();

    if let Err(e) = legacy::run(cli).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
