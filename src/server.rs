//! MCP server implementation and session state management.

use crate::stdlib::StdlibDocs;
use crate::tools::inspect_crate::{InspectCrateRequest, handle_inspect_crate};
use crate::tools::inspect_item::{InspectItemRequest, handle_inspect_item};
use crate::tools::search::{SearchRequest, handle_search};
use crate::tools::set_workspace::{format_response, handle_set_workspace};
use crate::worker::DocState;
use crate::workspace::WorkspaceContext;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars::{self, JsonSchema, generate::SchemaSettings},
    tool, tool_handler, tool_router,
};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

/// Legacy server context for backward compatibility.
///
/// This wraps DocState to provide the old API that tool handlers expect.
/// It's a transitional type that will be phased out as tools migrate to DocState.
#[derive(Debug, Clone)]
pub struct ServerContext {
    /// Reference to the shared DocState
    state: Arc<DocState>,
}

impl ServerContext {
    /// Create a new server context wrapping DocState.
    pub fn new(state: Arc<DocState>) -> Self {
        Self { state }
    }

    /// Get the underlying DocState.
    pub fn doc_state(&self) -> &Arc<DocState> {
        &self.state
    }

    /// Get the current working directory (blocking).
    pub fn working_directory(&self) -> Option<PathBuf> {
        // Use block_in_place for sync access from async context
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.state.working_directory())
        })
    }

    /// Get cached workspace context (blocking).
    pub fn workspace_context(&self) -> Option<WorkspaceContext> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.state.workspace())
        })
    }

    /// Get the Cargo.lock path (blocking).
    pub fn cargo_lock_path(&self) -> Option<PathBuf> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.state.cargo_lock_path())
        })
    }

    /// Get the stdlib documentation provider.
    pub fn stdlib(&self) -> Option<&Arc<StdlibDocs>> {
        self.state.stdlib()
    }

    /// Check if stdlib is available.
    pub fn has_stdlib(&self) -> bool {
        self.state.stdlib().is_some()
    }
}

/// Parameters for set_workspace tool
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetWorkspaceRequest {
    /// Path to the Rust project directory (must contain Cargo.toml)
    pub path: String,
}

/// MCP Server for Rust documentation queries
#[derive(Clone)]
pub struct ItemServer {
    /// Shared documentation state (cache, workspace, stdlib)
    state: Arc<DocState>,

    /// Tool router for handling MCP tool calls
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for ItemServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemServer")
            .field("state", &self.state)
            .finish()
    }
}

#[tool_router]
impl ItemServer {
    /// Create a new ItemServer with optional stdlib support.
    pub fn new(stdlib: Option<Arc<StdlibDocs>>) -> Self {
        Self {
            state: Arc::new(DocState::new(stdlib)),
            tool_router: Self::tool_router(),
        }
    }

    /// Get a reference to the shared DocState.
    pub fn doc_state(&self) -> &Arc<DocState> {
        &self.state
    }

    /// Get a ServerContext wrapper for legacy tool handler compatibility.
    pub fn server_context(&self) -> ServerContext {
        ServerContext::new(self.state.clone())
    }

    #[tool(
        description = "Configure the workspace path for a Rust project. Automatically discovers workspace members and resolves all dependencies with their versions using cargo metadata."
    )]
    async fn set_workspace(
        &self,
        Parameters(SetWorkspaceRequest { path }): Parameters<SetWorkspaceRequest>,
    ) -> std::result::Result<String, String> {
        // Get current workspace before changing it
        let old_workspace = self.state.working_directory().await;

        // Execute the logic, passing current workspace for change detection
        let (canonical_path, workspace_info, changed) =
            handle_set_workspace(path, old_workspace.as_deref())
                .await
                .map_err(|e| format!("Failed to set workspace: {}", e))?;

        // Update state
        let cargo_lock = canonical_path.join("Cargo.lock");
        let cargo_lock = if cargo_lock.exists() {
            Some(cargo_lock)
        } else {
            None
        };

        // Clear cache when workspace changes
        if changed {
            self.state.clear_cache().await;
        }

        self.state
            .set_workspace(canonical_path.clone(), workspace_info.clone(), cargo_lock)
            .await;

        // Format response with old workspace and changed flag
        let response = format_response(
            &canonical_path,
            &workspace_info,
            old_workspace.as_deref(),
            changed,
        );

        Ok(response)
    }

    #[tool(
        description = "Inspect crate-level information. Without a crate name, lists all crates with descriptions and stats. With a crate name, shows detailed structure including modules, exports, and item counts.",
        input_schema = inline_schema_for_type::<InspectCrateRequest>()
    )]
    async fn inspect_crate(
        &self,
        Parameters(request): Parameters<InspectCrateRequest>,
    ) -> std::result::Result<String, String> {
        let context = self.server_context();
        handle_inspect_crate(&context, request)
            .await
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Inspect a Rust item (struct, enum, function, trait, module, etc.) from the workspace or dependencies. Supports path queries like 'Vec', 'std::vec::Vec', or 'HashMap'. Returns formatted documentation with configurable detail levels.",
        input_schema = inline_schema_for_type::<InspectItemRequest>()
    )]
    async fn inspect_item(
        &self,
        Parameters(request): Parameters<InspectItemRequest>,
    ) -> std::result::Result<String, String> {
        let context = self.server_context();
        handle_inspect_item(&context, request)
            .await
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Search for Rust items within a crate using TF-IDF full-text search. Searches item names and documentation, returning ranked results by relevance.",
        input_schema = inline_schema_for_type::<SearchRequest>()
    )]
    async fn search(
        &self,
        Parameters(request): Parameters<SearchRequest>,
    ) -> std::result::Result<String, String> {
        let context = self.server_context();
        handle_search(&context, request)
            .await
            .map_err(|e| e.to_string())
    }
}

#[tool_handler]
impl ServerHandler for ItemServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "rustdoc-mcp: A focused Rust documentation server with beautiful syntax formatting. \
                 Automatically detects workspace and generates documentation on startup. \
                 Standard library (std, core, alloc) is always available if rust-docs-json is installed. \
                 Use set_workspace to override automatic detection if needed."
                    .to_string(),
            ),
        }
    }
}

/// Expands tilde (`~`) in a path to the user's home directory.
///
/// - `~/foo` becomes `/home/user/foo`
/// - `~` becomes `/home/user`
/// - Other paths are returned unchanged
///
/// Returns `Cow::Borrowed` if no expansion needed, `Cow::Owned` if expanded.
pub fn expand_tilde(path: &str) -> Cow<'_, str> {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return Cow::Owned(home.join(stripped).display().to_string());
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return Cow::Owned(home.display().to_string());
    }
    Cow::Borrowed(path)
}

/// Generate an inline JSON schema for MCP tools
///
/// Unlike rmcp's default `schema_for_type()`, this function sets `inline_subschemas = true`
/// to generate inline enum definitions instead of $ref patterns. This ensures MCP Inspector
/// displays enums as dropdown widgets rather than raw JSON input fields.
pub fn inline_schema_for_type<T: JsonSchema>() -> Arc<JsonObject> {
    let mut settings = SchemaSettings::draft07();
    settings.transforms = vec![Box::new(schemars::transform::AddNullable::default())];
    settings.inline_subschemas = true;

    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let object = serde_json::to_value(schema).expect("failed to serialize schema");

    let json_object = match object {
        serde_json::Value::Object(object) => object,
        _ => panic!("Schema serialization produced non-object value"),
    };

    Arc::new(json_object)
}
