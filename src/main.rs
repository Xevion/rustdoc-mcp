use rmcp::{ServiceExt, transport::stdio};
use rustdoc_mcp::server::ItemServer;
use rustdoc_mcp::stdlib::StdlibDocs;
use rustdoc_mcp::worker::spawn_background_worker;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustdoc_mcp::tracing::init();

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

    // Spawn background worker with cancellation support
    let worker_ctx = spawn_background_worker(server.doc_state().clone());
    tracing::debug!("Background worker spawned");

    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Error serving MCP server: {:?}", e);
    })?;

    // Wait for the MCP service to complete (client disconnect or error)
    service.waiting().await?;

    // Gracefully shut down the background worker
    tracing::info!("MCP service stopped, shutting down background worker");
    worker_ctx.shutdown().await;

    Ok(())
}
