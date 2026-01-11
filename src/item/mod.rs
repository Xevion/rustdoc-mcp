//! Item references and iterators for traversing documentation.

pub(crate) mod item_ref;
pub(crate) mod iterator;

// Internal re-exports
pub(crate) use item_ref::{ItemPath, ItemRef};
