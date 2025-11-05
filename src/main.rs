use cargo_doc_mcp::server::{ItemServer, spawn_workspace_detection};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up logging - write to stderr to avoid interfering with MCP protocol on stdout
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting cargo-doc-mcp MCP server");

    // Create the MCP server
    let server = ItemServer::new();

    // Spawn background task for workspace auto-detection
    spawn_workspace_detection(server.context()).await;

    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Error serving MCP server: {:?}", e);
    })?;

    // Wait for the service to complete
    service.waiting().await?;

    Ok(())
}
