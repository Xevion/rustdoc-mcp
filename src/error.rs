//! Centralized error handling with typed error enums.
//!
//! This module provides structured error types for all MCP tool operations.
//! Errors are designed to:
//! - Provide detailed context via Debug for logging (`{:?}`)
//! - Provide user-friendly messages via Display for MCP responses (`{}`)
//! - Enable pattern matching for programmatic error handling
//!
//! # Error Hierarchy
//!
//! ```text
//! ToolError (top-level)
//! ├── Config(ConfigError)    - Workspace configuration issues
//! ├── Load(LoadError)        - Documentation loading/generation
//! ├── Query(QueryError)      - Search and lookup operations
//! ├── Validation(ValidationError) - Input validation
//! └── Internal              - Unexpected internal errors
//! ```

use std::path::PathBuf;
use thiserror::Error;

use crate::types::CrateName;

/// A specialized Result type for rustdoc-mcp operations.
///
/// This is an alias for `anyhow::Result` with context added via `.context()` and
/// `.with_context()` methods throughout the codebase.
pub type Result<T> = anyhow::Result<T>;

/// Primary error type for MCP tool operations.
///
/// This enum represents all error conditions that can occur during tool execution.
/// Use the `From` impls to convert from more specific error types.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Workspace configuration errors (no workspace, invalid path, etc.)
    #[error("{0}")]
    Config(#[from] ConfigError),

    /// Documentation loading or generation errors
    #[error("{0}")]
    Load(#[from] LoadError),

    /// Query/search operation errors
    #[error("{0}")]
    Query(#[from] QueryError),

    /// Input validation errors
    #[error("{0}")]
    Validation(#[from] ValidationError),

    /// Internal/unexpected errors (formatting failures, etc.)
    #[error("Internal error: {message}")]
    Internal {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },
}

impl ToolError {
    /// Create an internal error from a message.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            source: None,
        }
    }

    /// Create an internal error with a source cause.
    pub fn internal_with_source(message: impl Into<String>, source: anyhow::Error) -> Self {
        Self::Internal {
            message: message.into(),
            source: Some(source),
        }
    }

    /// Get optional help text for this error.
    ///
    /// Returns additional guidance for resolving the error, if available.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Self::Config(e) => e.help(),
            Self::Load(e) => e.help(),
            Self::Query(e) => e.help(),
            Self::Validation(e) => e.help(),
            Self::Internal { .. } => None,
        }
    }

    /// Get a user-friendly message with optional help text appended.
    pub fn user_message(&self) -> String {
        match self.help() {
            Some(help) => format!("{}\n\n{}", self, help),
            None => self.to_string(),
        }
    }
}

impl From<std::fmt::Error> for ToolError {
    fn from(e: std::fmt::Error) -> Self {
        Self::Internal {
            message: "Formatting failed".to_string(),
            source: Some(e.into()),
        }
    }
}

/// Errors related to workspace configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No workspace has been configured.
    #[error("No workspace configured")]
    NoWorkspace,

    /// The specified path does not exist.
    #[error("Path does not exist: {path}")]
    PathNotFound { path: PathBuf },

    /// The path exists but is not a directory.
    #[error("Path is not a directory: {path}")]
    NotADirectory { path: PathBuf },

    /// No Cargo.toml found at or above the specified path.
    #[error("No Cargo.toml found at or above: {path}")]
    NoCargoToml { path: PathBuf },

    /// The path points to a file that cannot be used as a workspace.
    #[error("Cannot use {file_type} as workspace path: {path}")]
    InvalidFileType { path: PathBuf, file_type: String },

    /// Failed to read cargo metadata.
    #[error("Failed to read cargo metadata: {reason}")]
    CargoMetadata { reason: String },
}

impl ConfigError {
    /// Get help text for this error.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Self::NoWorkspace => Some(
                "To configure a workspace:\n\
                 • Use set_workspace with a path to a Rust project\n\n\
                 To enable standard library docs:\n\
                 • Run: rustup component add rust-docs-json --toolchain nightly",
            ),
            Self::PathNotFound { .. } => Some("Check that the path exists and is accessible."),
            Self::NotADirectory { .. } => Some("Provide a directory path, not a file path."),
            Self::NoCargoToml { .. } => Some("Ensure the directory contains a Cargo.toml file."),
            Self::InvalidFileType { .. } => {
                Some("Provide the project directory (containing Cargo.toml) instead.")
            }
            Self::CargoMetadata { .. } => Some(
                "Ensure:\n\
                 • The Cargo.toml is valid\n\
                 • All dependencies are available\n\
                 • cargo is installed and accessible",
            ),
        }
    }
}

/// Errors related to loading or generating documentation.
#[derive(Debug, Clone, Error)]
pub enum LoadError {
    /// Crate not found in the workspace.
    #[error("Crate '{crate_name}' not found in workspace")]
    CrateNotFound { crate_name: CrateName },

    /// Documentation file not found and cannot be generated.
    #[error("Documentation not found for '{crate_name}'")]
    NotFound { crate_name: CrateName },

    /// Documentation file not found at a specific path.
    #[error("Documentation not found for '{crate_name}' at {path}")]
    NotFoundAt {
        crate_name: CrateName,
        path: PathBuf,
    },

    /// Documentation generation failed.
    #[error("Failed to generate documentation for '{crate_name}': {reason}")]
    GenerationFailed {
        crate_name: CrateName,
        reason: String,
    },

    /// Failed to parse the documentation file.
    #[error("Failed to parse documentation for '{crate_name}': {reason}")]
    ParseFailed {
        crate_name: CrateName,
        reason: String,
    },
}

impl LoadError {
    /// Get help text for this error.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Self::CrateNotFound { .. } => {
                Some("Use inspect_crate without arguments to see available crates.")
            }
            Self::NotFound { .. } => Some(
                "The crate may need documentation generated.\n\
                 Use inspect_crate to see available crates.",
            ),
            Self::NotFoundAt { .. } => Some(
                "The documentation file may have been deleted or moved.\n\
                 Try rebuilding with: cargo +nightly rustdoc",
            ),
            Self::GenerationFailed { .. } => Some(
                "Ensure:\n\
                 • Nightly toolchain is installed (rustup install nightly)\n\
                 • The crate exists in your dependencies\n\
                 • The crate compiles successfully",
            ),
            Self::ParseFailed { .. } => Some(
                "The documentation JSON may be corrupted or incompatible.\n\
                 Try regenerating: cargo +nightly rustdoc",
            ),
        }
    }
}

/// Errors related to search and query operations.
#[derive(Debug, Error)]
pub enum QueryError {
    /// No items found matching the query.
    #[error("No items found matching '{query}'{}", kind.map(|k| format!(" with kind '{:?}'", k)).unwrap_or_default())]
    NotFound {
        query: String,
        kind: Option<crate::search::ItemKind>,
    },

    /// Empty query provided.
    #[error("Empty query provided")]
    EmptyQuery,

    /// Failed to build the search index.
    #[error("Failed to build search index for '{crate_name}': {reason}")]
    IndexBuildFailed {
        crate_name: CrateName,
        reason: String,
    },

    /// Item found but has wrong kind.
    #[error("Item '{query}' found but is not a {expected_kind:?}")]
    WrongKind {
        query: String,
        expected_kind: crate::search::ItemKind,
    },
}

impl QueryError {
    /// Get help text for this error.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Self::NotFound { .. } => Some(
                "Search tips:\n\
                 • Try a shorter or more general term\n\
                 • Search for types like 'HashMap', 'Vec', 'String'\n\
                 • Try function names like 'parse', 'read', 'write'\n\
                 • Search uses stemming: 'parsing' matches 'parse'",
            ),
            Self::EmptyQuery => Some("Provide a search term or item path."),
            Self::IndexBuildFailed { .. } => Some(
                "The crate documentation may be missing or corrupted.\n\
                 Try regenerating: cargo +nightly rustdoc",
            ),
            Self::WrongKind { .. } => {
                Some("Try searching without the kind filter to see all matching items.")
            }
        }
    }
}

/// Errors related to input validation.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Invalid crate name.
    #[error("{0}")]
    CrateName(#[from] CrateNameError),

    /// Invalid version string.
    #[error("Invalid version '{version}': {reason}")]
    Version { version: String, reason: String },
}

impl ValidationError {
    /// Get help text for this error.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Self::CrateName(e) => e.help(),
            Self::Version { .. } => {
                Some("Version must match semver format (e.g., '1.0.0', '0.1.2-alpha').")
            }
        }
    }
}

/// Error type for invalid crate names.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CrateNameError {
    /// Crate name is empty.
    #[error("crate name cannot be empty")]
    Empty,

    /// Crate name starts with an invalid character.
    #[error("crate name must start with a letter or underscore, got '{character}'")]
    InvalidStart { character: char },

    /// Crate name contains an invalid character.
    #[error("crate name contains invalid character '{character}'")]
    InvalidCharacter { character: char },
}

impl CrateNameError {
    /// Get help text for this error.
    pub fn help(&self) -> Option<&'static str> {
        Some(
            "Crate names must:\n\
             • Start with a letter or underscore\n\
             • Contain only letters, numbers, underscores, and hyphens",
        )
    }
}

/// Error type for hash parsing failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseHashError {
    /// Hash string contains invalid hexadecimal characters.
    #[error("invalid hexadecimal characters in hash")]
    InvalidHex,

    /// Hash string has incorrect length.
    #[error("invalid hash length: expected 16 or 64 hex characters, got {length}")]
    InvalidLength { length: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    #[test]
    fn test_tool_error_user_message() {
        let err = ToolError::Config(ConfigError::NoWorkspace);
        let msg = err.user_message();
        check!(msg.contains("No workspace configured"));
        check!(msg.contains("set_workspace"));
    }

    #[test]
    fn test_config_error_help() {
        let err = ConfigError::NoWorkspace;
        check!(err.help().is_some());
        check!(err.help().unwrap().contains("set_workspace"));
    }

    #[test]
    fn test_load_error_display() {
        let err = LoadError::NotFound {
            crate_name: CrateName::new_unchecked("serde"),
        };
        check!(err.to_string().contains("serde"));
    }

    #[test]
    fn test_query_error_with_kind() {
        let err = QueryError::NotFound {
            query: "Foo".to_string(),
            kind: Some(crate::search::ItemKind::Trait),
        };
        let msg = err.to_string();
        check!(msg.contains("Foo"));
        check!(msg.contains("Trait"));
    }

    #[test]
    fn test_crate_name_error_help() {
        let err = CrateNameError::Empty;
        check!(err.help().is_some());
    }
}
