//! Formatting utilities for documentation display.

pub mod builders;
pub mod extraction;
pub mod renderers;

use crate::CrateIndex;
use crate::error::Result;
use crate::format::extraction::TypeInfo;
use rmcp::schemars;
use serde::{Deserialize, Serialize};

// Re-export type building function and formatter trait
pub use builders::{TypeFormatter, build_type_syntax, extract_id_from_type};

/// Format a type definition using syn + prettyplease for consistent, beautiful output
pub fn format_type_with_detail_level(
    def: &TypeInfo,
    doc: &CrateIndex,
    _detail_level: DetailLevel,
) -> Result<String> {
    // Always use formatted output with prettyplease for consistency
    build_type_syntax(def, doc)
}

/// DetailLevel level for documentation display.
///
/// DO NOT add doc comments to individual variants - this causes schemars to generate
/// `oneOf` schemas instead of simple `enum` arrays, breaking MCP client enum handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DetailLevel {
    Low,
    Medium,
    High,
}

impl Default for DetailLevel {
    fn default() -> Self {
        Self::Medium
    }
}

/// Filter for controlling which items to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemFilter {
    /// Show only public items
    Public,
    /// Show only private items
    Private,
    /// Show only methods
    Methods,
    /// Show only fields
    Fields,
    /// Show only enum variants
    Variants,
}

/// Context for formatting documentation items.
#[derive(Debug, Clone)]
pub struct FormatOptions {
    detail_level: DetailLevel,
    include_source: bool,
    recursive: bool,
    filters: Vec<ItemFilter>,
}

impl FormatOptions {
    /// Create a new format context with default settings.
    pub fn new() -> Self {
        Self {
            detail_level: DetailLevel::default(),
            include_source: false,
            recursive: false,
            filters: Vec::new(),
        }
    }

    /// Set the detail_level level.
    pub fn with_detail_level(mut self, detail_level: DetailLevel) -> Self {
        self.detail_level = detail_level;
        self
    }

    /// Set whether to include source code.
    pub fn with_source(mut self, include_source: bool) -> Self {
        self.include_source = include_source;
        self
    }

    /// Set whether to recurse into child items.
    pub fn with_recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Set the filters to apply.
    pub fn with_filters(mut self, filters: Vec<ItemFilter>) -> Self {
        self.filters = filters;
        self
    }

    /// Get the detail_level level.
    pub fn detail_level(&self) -> DetailLevel {
        self.detail_level
    }

    /// Check if source code should be included.
    pub fn include_source(&self) -> bool {
        self.include_source
    }

    /// Check if recursion is enabled.
    pub fn recursive(&self) -> bool {
        self.recursive
    }

    /// Get the filters.
    pub fn filters(&self) -> &[ItemFilter] {
        &self.filters
    }

    /// Check if a specific filter is active.
    pub fn has_filter(&self, filter: ItemFilter) -> bool {
        self.filters.contains(&filter)
    }
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self::new()
    }
}
