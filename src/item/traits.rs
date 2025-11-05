//! Trait-related types and utilities.

use rustdoc_types::Id;

/// Information about a trait implementation.
#[derive(Debug, Clone)]
pub struct TraitImplInfo {
    pub trait_name: Option<String>,
    pub methods: Vec<Id>,
}
