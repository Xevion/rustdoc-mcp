//! QueryContext provides request-scoped context for documentation queries with automatic caching.
//! Also includes path parsing utilities for resolving item queries.

use crate::error::LoadError;
use crate::item::ItemRef;
use crate::search::rustdoc::CrateIndex;
use crate::workspace::WorkspaceContext;
use bumpalo::Bump;
use rapidfuzz::distance::jaro_winkler;
use rustdoc_types::{Id, Item, ItemEnum};
use std::borrow::Cow;
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
    sync::Arc,
};

/// Expands tilde (`~`) in a path to the user's home directory.
///
/// - `~/foo` becomes `/home/user/foo`
/// - `~` becomes `/home/user`
/// - Other paths are returned unchanged
///
/// Returns `Cow::Borrowed` if no expansion needed, `Cow::Owned` if expanded.
pub fn expand_tilde(path: &str) -> Cow<'_, str> {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return Cow::Owned(home.join(stripped).display().to_string());
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return Cow::Owned(home.display().to_string());
    }
    Cow::Borrowed(path)
}

/// Represents a parsed item path like `std::vec::Vec` or `MyStruct`
#[derive(Debug, Clone)]
pub struct QueryPath {
    /// The crate name if explicitly specified or resolved
    pub crate_name: Option<String>,
    /// Path components (modules and item name)
    pub path_components: Vec<String>,
}

impl QueryPath {
    /// Get the final item name (last component)
    pub fn item_name(&self) -> &str {
        self.path_components
            .last()
            .expect("QueryPath must have at least one component")
    }

    /// Get the module path without the item name
    pub fn module_path(&self) -> Option<String> {
        if self.path_components.len() > 1 {
            Some(self.path_components[..self.path_components.len() - 1].join("::"))
        } else {
            None
        }
    }

    /// Get the full path including module and item
    pub fn full_path(&self) -> String {
        self.path_components.join("::")
    }

    /// Get the fully qualified path including crate name
    pub fn qualified_path(&self) -> String {
        if let Some(ref crate_name) = self.crate_name {
            format!("{}::{}", crate_name, self.full_path())
        } else {
            self.full_path()
        }
    }
}

/// Parse an item path query into components
///
/// Examples:
/// - `Vec` → path_components=["Vec"]
/// - `std::vec::Vec` → path_components=["std", "vec", "Vec"]
/// - `collections::HashMap` → path_components=["collections", "HashMap"]
///
/// The crate name is resolved later with context knowledge of available crates
pub fn parse_item_path(query: &str) -> QueryPath {
    let parts: Vec<String> = query
        .split("::")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        // Handle empty query
        QueryPath {
            crate_name: None,
            path_components: vec!["".to_string()],
        }
    } else {
        QueryPath {
            crate_name: None,
            path_components: parts,
        }
    }
}

/// Attempt to resolve the crate name from the path using known crates
///
/// If the first component matches a known crate name, it's extracted as the crate
/// and removed from the path components.
///
/// Returns the resolved crate name if found.
pub fn resolve_crate_from_path(path: &mut QueryPath, known_crates: &[String]) -> Option<String> {
    if path.path_components.is_empty() {
        return None;
    }

    let first = &path.path_components[0];
    if known_crates.iter().any(|c| c == first) {
        // First component matches a known crate
        let crate_name = path.path_components.remove(0);
        path.crate_name = Some(crate_name.clone());
        Some(crate_name)
    } else {
        None
    }
}

/// A Send-safe wrapper around a raw pointer to bump-allocated data.
///
/// SAFETY INVARIANT: The pointer must remain valid for the lifetime of the QueryContext.
/// We enforce this by:
/// 1. Only creating ArenaPtr from arena allocations within QueryContext
/// 2. Never exposing ArenaPtr outside QueryContext's lifetime
/// 3. QueryContext is not Clone, preventing arena from being freed while refs exist
struct ArenaPtr<T> {
    ptr: NonNull<T>,
    // Ensure we're not Send/Sync unless T is
    _marker: PhantomData<*const T>,
}

// SAFETY: ArenaPtr can be Send because:
// 1. The arena allocation lives as long as QueryContext
// 2. QueryContext is not Clone - when it drops, all ArenaPtr become invalid
// 3. We only dereference through &QueryContext methods, ensuring lifetime bounds
unsafe impl<T> Send for ArenaPtr<T> where T: Send {}

impl<T> ArenaPtr<T> {
    /// Create a new ArenaPtr from a reference.
    /// SAFETY: The reference must remain valid for the lifetime of the containing QueryContext.
    fn new(reference: &T) -> Self {
        Self {
            ptr: NonNull::from(reference),
            _marker: PhantomData,
        }
    }

    /// Dereference the pointer with proper lifetime bounds.
    /// SAFETY: Caller must ensure the reference doesn't outlive the arena allocation.
    unsafe fn as_ref<'a>(&self) -> &'a T {
        // SAFETY: Upheld by caller - reference is valid for QueryContext lifetime
        unsafe { self.ptr.as_ref() }
    }
}

/// Represents a single query context with its own cache and state.
/// Automatically cleans up when dropped.
pub struct QueryContext {
    workspace: Arc<WorkspaceContext>,
    /// Bump allocator for request-scoped memory management
    arena: Bump,
    /// Per-query cache of loaded documentation indices
    doc_cache: RefCell<HashMap<String, ArenaPtr<CrateIndex>>>,
}

impl Debug for QueryContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryContext")
            .field("workspace", &self.workspace.root)
            .field("doc_cache_len", &self.doc_cache.borrow().len())
            .finish()
    }
}

impl QueryContext {
    /// Create a new query context for the given workspace.
    pub fn new(workspace: Arc<WorkspaceContext>) -> Self {
        Self {
            workspace,
            arena: Bump::new(),
            doc_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Get the workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace.root
    }

    /// Load a crate's documentation by name, using the cache if available.
    ///
    /// Automatically generates documentation if it doesn't exist or is stale.
    /// Returns a reference bound to the lifetime of this QueryContext.
    pub fn load_crate(&self, crate_name: &str) -> Result<&CrateIndex, LoadError> {
        // Check cache first and return reference with proper lifetime
        if let Some(cached_ptr) = self.doc_cache.borrow().get(crate_name) {
            // SAFETY: The ArenaPtr is valid for the lifetime of self (arena allocation).
            // We control all access through &self methods, ensuring the reference cannot outlive QueryContext.
            return Ok(unsafe { cached_ptr.as_ref() });
        }

        // Normalize crate name (replace dashes with underscores for file lookup)
        let normalized_name = crate_name.replace('-', "_");

        // Try to find and load the JSON doc file
        let doc_path = self
            .workspace
            .root
            .join("target/doc")
            .join(format!("{}.json", normalized_name));

        // Determine if this is a workspace member or external dependency
        let is_workspace_member = self.workspace.members.contains(&crate_name.to_string());
        let version = self.workspace.get_version(crate_name);

        // Find Cargo.lock path
        let cargo_lock_path = self.workspace.root.join("Cargo.lock");
        let cargo_lock_path = if cargo_lock_path.exists() {
            Some(cargo_lock_path)
        } else {
            None
        };

        // If documentation doesn't exist or needs regeneration, generate it
        if !doc_path.exists() {
            tracing::info!(
                "Documentation not found for '{}', generating...",
                crate_name
            );

            // Use block_in_place to allow blocking within async context
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    crate::workspace::get_docs(
                        crate_name,
                        version,
                        &self.workspace.root,
                        is_workspace_member,
                        cargo_lock_path.as_deref(),
                    )
                    .await
                })
            });

            // Handle generation errors
            if let Err(e) = result {
                tracing::error!("Failed to generate docs for '{}': {}", crate_name, e);
                return Err(LoadError::ParseError {
                    crate_name: crate_name.to_string(),
                    error: format!("Failed to generate documentation: {}", e),
                });
            }
        }

        // Load the documentation (either existing or just generated)
        let crate_index = CrateIndex::load(&doc_path).map_err(|e| {
            tracing::error!("Failed to load docs for '{}': {}", crate_name, e);
            LoadError::ParseError {
                crate_name: crate_name.to_string(),
                error: e.to_string(),
            }
        })?;

        // Allocate in arena and store Send-safe pointer in cache
        let allocated: &CrateIndex = self.arena.alloc(crate_index);
        let arena_ptr = ArenaPtr::new(allocated);

        self.doc_cache
            .borrow_mut()
            .insert(crate_name.to_string(), arena_ptr);

        // Return reference bound to self's lifetime
        Ok(allocated)
    }

    /// Resolve a path like "crate_name::module::Item" to an ItemRef.
    /// Populates suggestions if the path cannot be resolved.
    pub fn resolve_path<'a>(
        &'a self,
        path: &str,
        suggestions: &mut Vec<PathSuggestion<'a>>,
    ) -> Option<ItemRef<'a, Item>> {
        // Split path into crate name and remainder
        let (crate_name, index) = if let Some(index) = path.find("::") {
            (&path[..index], Some(index + 2))
        } else {
            (path, None)
        };

        // Load the crate
        let crate_index = match self.load_crate(crate_name) {
            Ok(index) => index,
            Err(_) => {
                // Generate suggestions for available crates
                suggestions.extend(
                    self.workspace
                        .members
                        .iter()
                        .map(|s| s.as_str())
                        .chain(self.workspace.dependency_names())
                        .map(|name| PathSuggestion {
                            path: name.to_string(),
                            item: None,
                            score: jaro_winkler::similarity(crate_name.chars(), name.chars()),
                        }),
                );
                return None;
            }
        };

        // Get the root module
        let root_item = crate_index.root_module()?;
        let item = ItemRef::builder(self, crate_index, root_item).build();

        // If there's more path to resolve, recurse through children
        if let Some(index) = index {
            self.find_children_recursive(item, path, index, suggestions)
        } else {
            Some(item)
        }
    }

    /// Recursively traverse the module tree to find an item by path.
    fn find_children_recursive<'a>(
        &'a self,
        item: ItemRef<'a, Item>,
        path: &str,
        index: usize,
        suggestions: &mut Vec<PathSuggestion<'a>>,
    ) -> Option<ItemRef<'a, Item>> {
        let remaining = &path[path.len().min(index)..];
        if remaining.is_empty() {
            return Some(item);
        }

        // Extract the next segment
        let segment_end = remaining
            .find("::")
            .map(|x| index + x)
            .unwrap_or(path.len());
        let segment = &path[index..segment_end];
        let next_segment_start = path.len().min(segment_end + 2);

        tracing::trace!(
            "Searching for '{}' in {} ({:?}), remaining: '{}'",
            segment,
            &path[..index],
            item.kind(),
            &path[next_segment_start..]
        );

        // Search through child items
        for child in item.children().build() {
            if let Some(name) = child.name()
                && name == segment
                && let Some(child) =
                    self.find_children_recursive(child, path, next_segment_start, suggestions)
            {
                return Some(child);
            }
        }

        // No match found - generate suggestions
        suggestions.extend(self.generate_suggestions(item, path, index));
        None
    }

    /// Generate fuzzy suggestions for items that are similar to the query.
    fn generate_suggestions<'a>(
        &'a self,
        item: ItemRef<'a, Item>,
        path: &str,
        index: usize,
    ) -> impl Iterator<Item = PathSuggestion<'a>> {
        item.children().build().filter_map(move |child| {
            child.name().and_then(|name| {
                let full_path = format!("{}{}", &path[..index], name);
                // Don't suggest paths that are prefixes of the query
                if path.starts_with(&full_path) {
                    None
                } else {
                    let score = jaro_winkler::similarity(path.chars(), full_path.chars());
                    Some(PathSuggestion {
                        path: full_path,
                        score,
                        item: Some(child),
                    })
                }
            })
        })
    }

    /// Get an item by its ID within a specific doc index.
    pub fn get_item<'a>(
        &'a self,
        crate_index: &'a CrateIndex,
        id: &Id,
    ) -> Option<ItemRef<'a, Item>> {
        crate_index
            .get_item(id)
            .map(|item| ItemRef::builder(self, crate_index, item).build())
    }

    /// Resolve a path of IDs to a final item (used for following re-exports).
    pub fn get_item_from_id_path<'a>(
        &'a self,
        crate_name: &str,
        ids: &[u32],
    ) -> Option<(ItemRef<'a, Item>, Vec<&'a str>)> {
        let mut path_segments = vec![];
        let crate_index = self.load_crate(crate_name).ok()?;

        let root = crate_index.root_module()?;
        let mut item = ItemRef::builder(self, crate_index, root).build();

        if let Some(name) = item.name() {
            path_segments.push(name);
        }

        for id in ids {
            item = item.get(&Id(*id))?;

            // Handle re-exports
            if let ItemEnum::Use(use_item) = item.inner() {
                if let Some(target_id) = &use_item.id {
                    item = item
                        .get(target_id)
                        .or_else(|| self.resolve_path(&use_item.source, &mut vec![]))?;
                }

                if !use_item.is_glob {
                    path_segments.push(&use_item.name);
                }
            } else if let Some(name) = item.name() {
                path_segments.push(name);
            }
        }

        Some((item, path_segments))
    }
}

/// Automatic cleanup when query context ends.
impl Drop for QueryContext {
    fn drop(&mut self) {
        tracing::trace!(
            "QueryContext dropped, cleaned up {} crates",
            self.doc_cache.borrow().len()
        );
    }
}

/// A fuzzy path suggestion with relevance score.
#[derive(Debug, Clone)]
pub struct PathSuggestion<'a> {
    pub path: String,
    pub item: Option<ItemRef<'a, Item>>,
    pub score: f64,
}

impl<'a> PathSuggestion<'a> {
    /// Get the suggested path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Get the associated item if available.
    pub fn item(&self) -> Option<ItemRef<'a, Item>> {
        self.item
    }

    /// Get the relevance score (0.0 to 1.0, higher is better).
    pub fn score(&self) -> f64 {
        self.score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use rstest::rstest;

    #[rstest]
    #[case("Vec", "Vec", None, "Vec")]
    #[case("std::vec::Vec", "Vec", Some("std::vec"), "std::vec::Vec")]
    fn test_parse_item_path(
        #[case] input: &str,
        #[case] expected_item: &str,
        #[case] expected_module: Option<&str>,
        #[case] expected_full: &str,
    ) {
        let path = parse_item_path(input);
        check!(path.item_name() == expected_item);
        check!(path.module_path() == expected_module.map(String::from));
        check!(path.full_path() == expected_full);
        check!(path.crate_name.is_none());
    }

    #[rstest]
    #[case("std::vec::Vec", &["std", "tokio"], Some("std"), "Vec", Some("vec"))]
    #[case("collections::HashMap", &["std", "tokio"], None, "HashMap", Some("collections"))]
    fn test_resolve_crate_from_path(
        #[case] input: &str,
        #[case] known_crates: &[&str],
        #[case] expected_crate: Option<&str>,
        #[case] expected_item: &str,
        #[case] expected_module: Option<&str>,
    ) {
        let mut path = parse_item_path(input);
        let known_crates_vec: Vec<String> = known_crates.iter().map(|s| s.to_string()).collect();

        let crate_name = resolve_crate_from_path(&mut path, &known_crates_vec);

        check!(crate_name == expected_crate.map(String::from));
        check!(path.crate_name == expected_crate.map(String::from));
        check!(path.item_name() == expected_item);

        if expected_crate.is_some() {
            check!(path.module_path() == expected_module.map(String::from));
        }
    }
}
