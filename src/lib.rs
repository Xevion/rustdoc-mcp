#![warn(unreachable_pub)]
#![warn(dead_code)]

pub mod cache;
pub mod error;
pub mod format;
pub mod item;
pub mod search;
pub mod server;
pub mod stdlib;
pub mod tools;
pub mod tracing;
pub mod types;
pub mod worker;
pub mod workspace;

// Re-export common types
pub use error::{
    ConfigError, CrateNameError, LoadError, ParseHashError, QueryError, Result, ToolError,
    ValidationError,
};
pub use format::{DetailLevel, TypeFormatter};
pub use search::{ItemKind, QueryContext};
pub use types::CrateName;
pub use worker::DocState;
pub use workspace::{CrateMetadata, CrateOrigin, WorkspaceContext};
