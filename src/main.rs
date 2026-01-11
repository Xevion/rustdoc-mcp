use rmcp::{ServiceExt, transport::stdio};
use rustdoc_mcp::server::ItemServer;
use rustdoc_mcp::stdlib::StdlibDocs;
use rustdoc_mcp::worker::spawn_background_worker;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up logging - write to stderr to avoid interfering with MCP protocol on stdout
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting rustdoc-mcp MCP server");

    // Discover stdlib documentation (optional - server works without it)
    let stdlib = match StdlibDocs::discover() {
        Ok(stdlib) => {
            tracing::info!(
                "Standard library docs available ({})",
                stdlib.rustc_version()
            );
            Some(Arc::new(stdlib))
        }
        Err(e) => {
            tracing::warn!(
                "Standard library docs not available: {}. \
                 Install with: rustup component add rust-docs-json --toolchain nightly",
                e
            );
            None
        }
    };

    // Create the MCP server with stdlib support
    let server = ItemServer::new(stdlib);

    // Spawn background worker for continuous workspace detection and doc generation
    let _worker_handle = spawn_background_worker(server.doc_state().clone());
    tracing::debug!("Background worker spawned");

    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Error serving MCP server: {:?}", e);
    })?;

    // Wait for the service to complete
    service.waiting().await?;

    Ok(())
}
