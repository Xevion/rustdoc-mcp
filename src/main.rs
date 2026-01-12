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
