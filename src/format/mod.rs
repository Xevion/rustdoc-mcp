//! Formatting utilities for documentation display.

mod builders;
pub(crate) mod renderers;

use rmcp::schemars;
use serde::{Deserialize, Serialize};

// Re-exports
pub use builders::TypeFormatter;

/// DetailLevel level for documentation display.
///
/// DO NOT add doc comments to individual variants - this causes schemars to generate
/// `oneOf` schemas instead of simple `enum` arrays, breaking MCP client enum handling.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    Low,
    #[default]
    Medium,
    High,
}
