use cargo_doc_mcp::context::ServerContext;
use cargo_doc_mcp::tools::set_workspace::{execute_set_workspace, format_response};
use cargo_doc_mcp::tools::list_crates::{execute_list_crates, ListCratesRequest};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

/// Parameters for set_workspace tool
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetWorkspaceRequest {
    /// Path to the Rust project directory (must contain Cargo.toml)
    pub path: String,
}

/// MCP Server for Rust documentation queries
#[derive(Clone)]
pub struct DocServer {
    /// Server context (working directory, workspace info)
    state: Arc<Mutex<ServerContext>>,

    /// Tool router for handling MCP tool calls
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl DocServer {
    /// Create a new DocServer instance
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ServerContext::new())),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Configure the workspace path for a Rust project. Automatically discovers workspace members and resolves all dependencies with their versions using cargo metadata."
    )]
    fn set_workspace(
        &self,
        Parameters(SetWorkspaceRequest { path }): Parameters<SetWorkspaceRequest>,
    ) -> Result<String, String> {
        // Execute the logic
        let (canonical_path, workspace_info) = execute_set_workspace(path)
            .map_err(|e| format!("Failed to set workspace: {}", e))?;

        // Update context
        {
            let mut state = self.state
                .lock()
                .unwrap_or_else(|_poisoned| {
                    tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                    std::process::abort();
                });
            state
                .set_working_directory(canonical_path.clone())
                .map_err(|e| format!("cargo-doc-mcp: Failed to update context: {}", e))?;
            state.set_workspace_metadata(workspace_info.clone());
        }

        // Format response
        let response = format_response(&canonical_path, &workspace_info);

        Ok(response)
    }

    #[tool(
        description = "List all crates available in the workspace. Shows workspace members and their dependencies with resolved version numbers in a simple, flat format."
    )]
    fn list_crates(
        &self,
        Parameters(request): Parameters<ListCratesRequest>,
    ) -> Result<String, String> {
        // Get context
        let state = self.state
            .lock()
            .unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });

        // Execute the logic
        execute_list_crates(&state, request).map_err(|e| format!("cargo-doc-mcp: {}", e))
    }
}

#[tool_handler]
impl ServerHandler for DocServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "cargo-doc-mcp: A focused Rust documentation server with beautiful syntax formatting. Start by using set_workspace to configure your project."
                    .to_string(),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up logging - write to stderr to avoid interfering with MCP protocol on stdout
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting cargo-doc-mcp MCP server");

    // Create and serve the MCP server over stdio
    let server = DocServer::new();
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Error serving MCP server: {:?}", e);
    })?;

    // Wait for the service to complete
    service.waiting().await?;

    Ok(())
}
