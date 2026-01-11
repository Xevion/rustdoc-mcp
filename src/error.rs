//! Error handling types and utilities.

use std::path::PathBuf;

use crate::types::CrateName;

/// A specialized Result type for rustdoc-mcp operations.
///
/// This is an alias for `anyhow::Result` with context added via `.context()` and
/// `.with_context()` methods throughout the codebase.
pub type Result<T> = anyhow::Result<T>;

/// Error returned when loading crate documentation fails.
#[derive(Debug, Clone)]
pub enum LoadError {
    /// Crate documentation not found and cannot be generated.
    NotFound { crate_name: CrateName },
    /// Documentation file not found at the expected path.
    NotFoundAt {
        crate_name: CrateName,
        path: PathBuf,
    },
    /// Failed to generate documentation (e.g., rustdoc command failed).
    GenerationFailed {
        crate_name: CrateName,
        error: String,
    },
    /// Failed to load or parse the documentation file.
    ParseError {
        crate_name: CrateName,
        error: String,
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { crate_name } => {
                write!(f, "Documentation not found for '{}'", crate_name)
            }
            Self::NotFoundAt { crate_name, path } => {
                write!(
                    f,
                    "Documentation not found for '{}' at {}",
                    crate_name,
                    path.display()
                )
            }
            Self::GenerationFailed { crate_name, error } => {
                write!(
                    f,
                    "Failed to generate documentation for '{}': {}",
                    crate_name, error
                )
            }
            Self::ParseError { crate_name, error } => {
                write!(f, "Failed to load docs for '{}': {}", crate_name, error)
            }
        }
    }
}

impl std::error::Error for LoadError {}
