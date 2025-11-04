//! Server context management for tracking workspace state and metadata.

use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Server context for the MCP server.
///
/// Maintains the current workspace location and cached metadata across tool invocations.
/// This is intentionally simple - no sessions, no persistence, just in-memory state.
#[derive(Debug, Default)]
pub struct ServerContext {
    /// Current working directory (project root)
    working_directory: Option<PathBuf>,

    /// Cached workspace metadata from cargo
    workspace_metadata: Option<WorkspaceMetadata>,
}

/// Metadata about a Rust workspace discovered via cargo metadata.
///
/// Contains workspace members, dependencies, and their resolved versions.
#[derive(Debug, Clone)]
pub struct WorkspaceMetadata {
    /// Workspace root path
    pub root: PathBuf,

    /// Workspace members (package names)
    pub members: Vec<String>,

    /// All dependencies with their resolved versions (name, version)
    pub dependencies: Vec<(String, String)>,
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
    pub fn set_working_directory(&mut self, path: PathBuf) -> Result<()> {
        // Validate the path exists
        if !path.exists() {
            return Err(anyhow!("Path does not exist: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(anyhow!("Path is not a directory: {}", path.display()));
        }

        // Clear cached workspace metadata when directory changes
        self.workspace_metadata = None;
        self.working_directory = Some(path);

        Ok(())
    }

    /// Get cached workspace metadata, if available
    pub fn workspace_metadata(&self) -> Option<&WorkspaceMetadata> {
        self.workspace_metadata.as_ref()
    }

    /// Set workspace metadata (typically called after running cargo metadata)
    pub fn set_workspace_metadata(&mut self, metadata: WorkspaceMetadata) {
        self.workspace_metadata = Some(metadata);
    }

    /// Resolve a path relative to the workspace root.
    ///
    /// Supports tilde expansion and validates that resolved paths stay within
    /// the workspace boundaries to prevent path traversal attacks.
    ///
    /// # Security
    /// Validates path containment before and after canonicalization to prevent
    /// symlink-based escapes from the workspace directory.
    pub fn resolve_workspace_path(&self, path: &str) -> Result<PathBuf> {
        let path_buf = PathBuf::from(&*shellexpand::tilde(path));

        // Resolve to absolute path first (without following symlinks yet)
        let resolved = if path_buf.is_absolute() {
            path_buf
        } else {
            match &self.working_directory {
                Some(wd) => wd.join(path_buf),
                None => {
                    return Err(anyhow!(
                        "Workspace not configured. Use set_workspace tool first or provide an absolute path."
                    ))
                }
            }
        };

        // Verify path is within workspace before canonicalization
        if let Some(wd) = &self.working_directory {
            if !resolved.starts_with(wd) {
                return Err(anyhow!(
                    "cargo-doc-mcp: Path '{}' is outside workspace boundaries",
                    resolved.display()
                ));
            }
        }

        // Now safe to canonicalize (follows symlinks)
        let canonical = std::fs::canonicalize(&resolved)
            .map_err(|e| anyhow!("Failed to canonicalize path '{}': {}", resolved.display(), e))?;

        // Verify again after canonicalization to catch symlink escapes
        if let Some(wd) = &self.working_directory {
            let canonical_wd = std::fs::canonicalize(wd)
                .map_err(|e| anyhow!("Failed to canonicalize workspace directory: {}", e))?;

            if !canonical.starts_with(&canonical_wd) {
                return Err(anyhow!(
                    "cargo-doc-mcp: Path '{}' is outside workspace boundaries",
                    canonical.display()
                ));
            }
        }

        Ok(canonical)
    }
}
