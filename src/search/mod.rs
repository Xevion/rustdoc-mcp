//! Full-text search infrastructure for rustdoc documentation.
//!
//! This module provides TF-IDF based search capabilities across Rust documentation,
//! including tokenization, indexing, scoring, and query resolution.

// Module declarations
mod index;
pub mod query;
pub mod rustdoc;
mod scoring;
mod tokenize;

// Re-exports for public API
pub use index::*;
pub use query::*;
pub use rustdoc::{
    CrateIndex, ItemKind, TraitImplInfo, item_enum_to_kind, item_kind_str, matches_kind,
};
pub use scoring::*;
pub use tokenize::*;
