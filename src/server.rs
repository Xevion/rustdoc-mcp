//! MCP server implementation and session state management.

use crate::tools::inspect_crate::{InspectCrateRequest, handle_inspect_crate};
use crate::tools::inspect_item::{InspectItemRequest, handle_inspect_item};
use crate::tools::search::{SearchRequest, handle_search};
use crate::tools::set_workspace::{format_response, handle_set_workspace};
use crate::workspace::{WorkspaceContext, auto_detect_workspace};
use anyhow::anyhow;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars::{self, JsonSchema, generate::SchemaSettings},
    tool, tool_handler, tool_router,
};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Server context for the MCP server.
///
/// Maintains the current workspace location and cached metadata across tool invocations.
/// This is intentionally simple - no sessions, no persistence, just in-memory state.
#[derive(Debug, Default, Clone)]
pub struct ServerContext {
    /// Current working directory (workspace root)
    working_directory: Option<PathBuf>,

    /// Cached workspace context from cargo
    workspace_context: Option<WorkspaceContext>,

    /// Path to Cargo.lock file (for dependency fingerprinting)
    cargo_lock_path: Option<PathBuf>,
}

impl ServerContext {
    /// Create a new server context
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current working directory
    pub fn working_directory(&self) -> Option<&PathBuf> {
        self.working_directory.as_ref()
    }

    /// Set the working directory and clear cached data
    pub fn set_working_directory(&mut self, path: PathBuf) -> anyhow::Result<()> {
        // Validate the path exists
        if !path.exists() {
            return Err(anyhow!("Path does not exist: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(anyhow!("Path is not a directory: {}", path.display()));
        }

        // Look for Cargo.lock in the directory
        let lock_path = path.join("Cargo.lock");
        self.cargo_lock_path = if lock_path.exists() {
            Some(lock_path)
        } else {
            None
        };

        // Clear cached workspace context when directory changes
        self.workspace_context = None;
        self.working_directory = Some(path);

        Ok(())
    }

    /// Get the Cargo.lock path if available
    pub fn cargo_lock_path(&self) -> Option<&PathBuf> {
        self.cargo_lock_path.as_ref()
    }

    /// Get cached workspace context, if available
    pub fn workspace_context(&self) -> Option<&WorkspaceContext> {
        self.workspace_context.as_ref()
    }

    /// Set workspace context (typically called after running cargo metadata)
    pub fn set_workspace_context(&mut self, context: WorkspaceContext) {
        self.workspace_context = Some(context);
    }

    /// Resolve a path relative to the workspace root.
    ///
    /// Supports tilde expansion and validates that resolved paths stay within
    /// the workspace boundaries to prevent path traversal attacks.
    ///
    /// # Security
    /// Canonicalizes the path first (resolving symlinks and normalizing), then validates
    /// that the canonical path is within workspace boundaries. This prevents symlink-based
    /// escapes and path traversal attacks.
    pub fn resolve_workspace_path(&self, path: &str) -> anyhow::Result<PathBuf> {
        let path_buf = PathBuf::from(&*expand_tilde(path));

        // Resolve to absolute path first
        let resolved = if path_buf.is_absolute() {
            path_buf
        } else {
            match &self.working_directory {
                Some(wd) => wd.join(path_buf),
                None => {
                    return Err(anyhow!(
                        "Workspace not configured. Use set_workspace tool first."
                    ));
                }
            }
        };

        // Canonicalize first (resolve symlinks, make absolute, normalize)
        let canonical = std::fs::canonicalize(&resolved).map_err(|e| {
            anyhow!(
                "Failed to resolve path '{}': {} (path may not exist or is inaccessible)",
                resolved.display(),
                e
            )
        })?;

        // Validate after canonicalization to catch symlink escapes
        if let Some(wd) = &self.working_directory {
            let canonical_wd = std::fs::canonicalize(wd)
                .map_err(|e| anyhow!("Failed to canonicalize workspace directory: {}", e))?;

            if !canonical.starts_with(&canonical_wd) {
                return Err(anyhow!(
                    "Path '{}' is outside workspace boundaries",
                    canonical.display()
                ));
            }
        }

        Ok(canonical)
    }
}

/// Expands tilde (`~`) in a path to the user's home directory.
///
/// - `~/foo` becomes `/home/user/foo`
/// - `~` becomes `/home/user`
/// - Other paths are returned unchanged
///
/// Returns `Cow::Borrowed` if no expansion needed, `Cow::Owned` if expanded.
fn expand_tilde(path: &str) -> Cow<'_, str> {
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

/// Parameters for set_workspace tool
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SetWorkspaceRequest {
    /// Path to the Rust project directory (must contain Cargo.toml)
    pub path: String,
}

/// MCP Server for Rust documentation queries
#[derive(Clone)]
pub struct ItemServer {
    /// Server context (working directory, workspace info)
    context: Arc<Mutex<ServerContext>>,

    /// Tool router for handling MCP tool calls
    tool_router: ToolRouter<Self>,
}

impl Default for ItemServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl ItemServer {
    /// Create a new ItemServer instance
    pub fn new() -> Self {
        Self {
            context: Arc::new(Mutex::new(ServerContext::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// Get a clone of the server context for background tasks
    pub fn context(&self) -> Arc<Mutex<ServerContext>> {
        self.context.clone()
    }

    #[tool(
        description = "Configure the workspace path for a Rust project. Automatically discovers workspace members and resolves all dependencies with their versions using cargo metadata."
    )]
    async fn set_workspace(
        &self,
        Parameters(SetWorkspaceRequest { path }): Parameters<SetWorkspaceRequest>,
    ) -> std::result::Result<String, String> {
        // Get current workspace before changing it
        let old_workspace = {
            let state = self.context.lock().unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });
            state.working_directory().cloned()
        };

        // Execute the logic, passing current workspace for change detection
        let (canonical_path, workspace_info, changed) =
            handle_set_workspace(path, old_workspace.as_deref())
                .await
                .map_err(|e| format!("Failed to set workspace: {}", e))?;

        // Update context
        {
            let mut state = self.context.lock().unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });
            state
                .set_working_directory(canonical_path.clone())
                .map_err(|e| format!("Failed to update context: {}", e))?;
            state.set_workspace_context(workspace_info.clone());
        }

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
        // Clone context to avoid holding lock across await
        let state = {
            let guard = self.context.lock().unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });
            guard.clone()
        };

        // Execute the logic
        handle_inspect_crate(&state, request)
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
        // Clone context to avoid holding lock across await
        let state = {
            let guard = self.context.lock().unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });
            guard.clone()
        };

        // Execute the logic
        handle_inspect_item(&state, request)
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
        // Clone context to avoid holding lock across await
        let state = {
            let guard = self.context.lock().unwrap_or_else(|_poisoned| {
                tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                std::process::abort();
            });
            guard.clone()
        };

        // Execute the logic
        handle_search(&state, request)
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
                "cargo-doc-mcp: A focused Rust documentation server with beautiful syntax formatting. Automatically detects workspace on startup. Use set_workspace to override if needed."
                    .to_string(),
            ),
        }
    }
}

/// Spawn background task for automatic workspace detection
pub async fn spawn_workspace_detection(context: Arc<Mutex<ServerContext>>) {
    tokio::spawn(async move {
        if let Some(workspace_path) = auto_detect_workspace().await {
            tracing::debug!("Attempting to configure auto-detected workspace");

            // Attempt to configure the workspace using the existing validation logic
            // Pass None for current workspace since this is initial auto-detection
            match handle_set_workspace(workspace_path.display().to_string(), None).await {
                Ok((canonical_path, workspace_info, _changed)) => {
                    // Update context with auto-detected workspace
                    let mut state = context.lock().unwrap_or_else(|_poisoned| {
                        tracing::error!("cargo-doc-mcp: Context state corrupted, aborting");
                        std::process::abort();
                    });

                    if let Err(e) = state.set_working_directory(canonical_path.clone()) {
                        tracing::warn!(
                            "Auto-detected workspace but failed to set working directory: {}",
                            e
                        );
                        return;
                    }

                    state.set_workspace_context(workspace_info.clone());

                    tracing::info!(
                        "✓ Auto-detected and configured workspace: {} ({} members, {} total crates)",
                        canonical_path.display(),
                        workspace_info.members.len(),
                        workspace_info.crate_info.len()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Auto-detection found Cargo.toml but validation failed: {}",
                        e
                    );
                }
            }
        } else {
            tracing::debug!("No workspace auto-detected, waiting for explicit set_workspace call");
        }
    });
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
