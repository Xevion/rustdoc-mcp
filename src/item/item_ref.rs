//! ItemRef provides lifetime-bound smart pointers to documentation items with convenient access patterns.

use crate::search::{CrateIndex, QueryContext, item_enum_to_kind};
use rustdoc_types::{Id, Item, ItemEnum, ItemKind, ItemSummary};
use std::{
    fmt::{self, Debug, Display, Formatter},
    ops::Deref,
};

/// A smart pointer to a documentation item with lifetime-bound access to the doc index.
/// Provides transparent access to the underlying item via Deref.
pub struct ItemRef<'a, T> {
    crate_index: &'a CrateIndex,
    item: &'a T,
    query: &'a QueryContext,
    /// Optional custom name override (used for re-exports)
    override_name: Option<&'a str>,
}

// Manually implement Copy and Clone without requiring T: Copy,
// since ItemRef only contains references which are always Copy.
impl<'a, T> Copy for ItemRef<'a, T> {}

impl<'a, T> Clone for ItemRef<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

/// Builder for constructing ItemRef instances with optional configuration.
pub struct ItemRefBuilder<'a, T> {
    query: &'a QueryContext,
    crate_index: &'a CrateIndex,
    item: &'a T,
    override_name: Option<&'a str>,
}

impl<'a, T> ItemRefBuilder<'a, T> {
    /// Build the ItemRef instance.
    pub fn build(self) -> ItemRef<'a, T> {
        ItemRef {
            query: self.query,
            crate_index: self.crate_index,
            item: self.item,
            override_name: self.override_name,
        }
    }
}

impl<'a, T> From<&ItemRef<'a, T>> for &'a CrateIndex {
    fn from(value: &ItemRef<'a, T>) -> Self {
        value.crate_index
    }
}

impl<'a, T> From<ItemRef<'a, T>> for &'a CrateIndex {
    fn from(value: ItemRef<'a, T>) -> Self {
        value.crate_index
    }
}

impl<'a, T> Deref for ItemRef<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.item
    }
}

impl<'a> ItemRef<'a, Item> {
    /// Get the name of this item, preferring custom name over the item's own name.
    #[inline]
    pub fn name(&self) -> Option<&'a str> {
        self.override_name.or(self.item.name.as_deref())
    }

    /// Get the inner ItemEnum of this item.
    #[inline]
    pub fn inner(&self) -> &'a ItemEnum {
        &self.item.inner
    }

    /// Get the ItemKind for this item.
    #[inline]
    pub fn kind(&self) -> ItemKind {
        item_enum_to_kind(&self.item.inner)
    }

    /// Get the documentation comment for this item.
    #[inline]
    pub fn comment(&self) -> Option<&'a str> {
        self.item.docs.as_deref()
    }

    /// Check if this item is public.
    #[inline]
    pub fn is_public(&self) -> bool {
        matches!(self.item.visibility, rustdoc_types::Visibility::Public)
    }

    /// Get the path to this item if available.
    pub fn path(&self) -> Option<ItemPath<'a>> {
        self.crate_index.path(&self.id)
    }

    /// Get the fully qualified path as a String (e.g., "std::vec::Vec").
    pub fn path_string(&self) -> Option<String> {
        self.path().map(|p| p.to_string())
    }

    /// Get path segments as a slice (e.g., ["std", "vec", "Vec"]).
    pub fn path_segments(&self) -> Option<&'a [String]> {
        self.crate_index
            .paths()
            .get(&self.id)
            .map(|summary| summary.path.as_slice())
    }

    /// Check if this item is at the crate root (has no parent module).
    pub fn is_root(&self) -> bool {
        self.path_segments()
            .map(|segments| segments.len() <= 1)
            .unwrap_or(true)
    }

    /// Navigate to the parent module, if this item has one.
    pub fn parent(&self) -> Option<ItemRef<'a, Item>> {
        let segments = self.path_segments()?;
        if segments.len() <= 1 {
            return None;
        }

        // Search for parent by matching path
        let parent_path = &segments[..segments.len() - 1];
        self.crate_index
            .paths()
            .iter()
            .find(|(_, summary)| summary.path == parent_path)
            .and_then(|(id, _)| self.get(id))
    }

    /// Build a new ItemRef for a different item type using the same context.
    pub fn build_ref<U>(&self, inner: &'a U) -> ItemRef<'a, U> {
        ItemRef::builder(self.query, self.crate_index, inner).build()
    }

    /// If this is a re-export (Use item), resolve to the original item.
    /// Returns None if this is not a re-export or if resolution fails.
    pub fn resolve_use(&self) -> Option<ItemRef<'a, Item>> {
        if let ItemEnum::Use(use_item) = self.inner() {
            // Try to resolve using the ID first
            if let Some(id) = use_item.id
                && let Some(resolved) = self.get(&id)
            {
                return Some(resolved);
            }
            // Fall back to path resolution
            self.query().resolve_path(&use_item.source, &mut vec![])
        } else {
            None
        }
    }
}

impl<'a, T: Debug> Debug for ItemRef<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemRef")
            .field("crate_index", &"<CrateIndex>")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl<'a, T> ItemRef<'a, T> {
    /// Create a new builder for constructing an ItemRef.
    pub fn builder(
        query: &'a QueryContext,
        crate_index: impl Into<&'a CrateIndex>,
        item: &'a T,
    ) -> ItemRefBuilder<'a, T> {
        ItemRefBuilder {
            query,
            crate_index: crate_index.into(),
            item,
            override_name: None,
        }
    }

    /// Set a custom name on this ItemRef (for re-exports).
    /// This mutates the ItemRef in place.
    pub fn set_name(&mut self, name: &'a str) {
        self.override_name = Some(name);
    }

    /// Get access to the underlying CrateIndex.
    #[inline]
    pub fn crate_index(&self) -> &'a CrateIndex {
        self.crate_index
    }

    /// Get access to the QueryContext.
    #[inline]
    pub fn query(&self) -> &'a QueryContext {
        self.query
    }

    /// Resolve an Id to an ItemRef.
    #[inline]
    pub fn get(&self, id: &Id) -> Option<ItemRef<'a, Item>> {
        self.crate_index.get(self.query, id)
    }
}

impl<'a> ItemRef<'a, rustdoc_types::Use> {
    /// Get the name for a Use item (always has a name).
    pub fn name(self) -> &'a str {
        self.override_name.unwrap_or(&self.item.name)
    }
}

/// A path to a documentation item (sequence of module segments).
#[derive(Debug)]
pub struct ItemPath<'a>(&'a [String]);

impl<'a> From<&'a ItemSummary> for ItemPath<'a> {
    fn from(value: &'a ItemSummary) -> Self {
        Self(&value.path)
    }
}

impl Display for ItemPath<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("::")?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}
