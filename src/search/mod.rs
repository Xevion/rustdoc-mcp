//! Full-text search infrastructure for rustdoc documentation.
//!
//! This module provides TF-IDF based search capabilities across Rust documentation,
//! including tokenization, indexing, scoring, and query resolution.

// Module declarations
pub(crate) mod index;
pub(crate) mod query;
pub(crate) mod rustdoc;
pub(crate) mod scoring;
pub(crate) mod tokenize;

// Public re-exports (used via lib.rs)
pub use query::QueryContext;
pub use rustdoc::ItemKind;

// Internal re-exports
pub(crate) use index::{DetailedSearchResult, SearchMatch, TermIndex};
pub(crate) use query::{parse_item_path, resolve_crate_from_path};
pub(crate) use rustdoc::{CrateIndex, item_enum_to_kind, item_kind_str, matches_kind};
pub(crate) use scoring::path_canonicality_score;
