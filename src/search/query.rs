//! QueryContext provides request-scoped context for documentation queries with automatic caching.
//! Also includes path parsing utilities for resolving item queries.

use crate::error::LoadError;
use crate::item::ItemRef;
use crate::search::rustdoc::CrateIndex;
use crate::types::CrateName;
use crate::workspace::WorkspaceContext;
use bumpalo::Bump;
use rapidfuzz::distance::jaro_winkler;
use rustdoc_types::{Id, Item, ItemEnum};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
    sync::Arc,
};

/// Represents a parsed item path like `std::vec::Vec` or `MyStruct`
#[derive(Debug, Clone)]
pub(crate) struct QueryPath {
    /// The crate name if explicitly specified or resolved
    pub crate_name: Option<CrateName>,
    /// Path components (modules and item name)
    pub path_components: Vec<String>,
}

impl QueryPath {
    /// Get the full path including module and item
    pub(crate) fn full_path(&self) -> String {
        self.path_components.join("::")
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
pub(crate) fn parse_item_path(query: &str) -> QueryPath {
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
pub(crate) fn resolve_crate_from_path(
    path: &mut QueryPath,
    known_crates: &[CrateName],
) -> Option<CrateName> {
    if path.path_components.is_empty() {
        return None;
    }

    let first = &path.path_components[0];

    if let Some(matched_crate) = known_crates.iter().find(|c| c.matches(first)) {
        // First component matches a known crate
        let _removed = path.path_components.remove(0);
        path.crate_name = Some(matched_crate.clone());
        Some(matched_crate.clone())
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
    doc_cache: RefCell<HashMap<CrateName, ArenaPtr<CrateIndex>>>,
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
    /// Attempts to load existing documentation first. If not found and the environment
    /// supports doc generation (has Cargo.toml, source files, etc.), generates docs.
    /// Returns a reference bound to the lifetime of this QueryContext.
    pub fn load_crate(&self, crate_name: &str) -> Result<&CrateIndex, LoadError> {
        // Check cache first and return reference with proper lifetime
        if let Some(cached_ptr) = self.doc_cache.borrow().get(crate_name) {
            // SAFETY: The ArenaPtr is valid for the lifetime of self (arena allocation).
            // We control all access through &self methods, ensuring the reference cannot outlive QueryContext.
            return Ok(unsafe { cached_ptr.as_ref() });
        }

        // Try to find and load the JSON doc file
        let crate_name_typed = CrateName::new_unchecked(crate_name);
        let doc_path = crate_name_typed.doc_json_path(&self.workspace.root.join("target/doc"));

        // If documentation doesn't exist, check if we can generate it
        if !doc_path.exists() {
            // Check if we have the minimum requirements to generate docs
            if !self.can_generate_docs(crate_name) {
                tracing::debug!(
                    crate_name,
                    reason = "environment not suitable",
                    "Cannot generate docs"
                );
                return Err(LoadError::NotFound {
                    crate_name: crate_name_typed,
                });
            }

            tracing::info!(crate_name, "Documentation not found, generating");

            let is_workspace_member = self.workspace.members.iter().any(|m| m.matches(crate_name));
            let version = self.workspace.get_version(crate_name);

            let cargo_lock_path = self.workspace.root.join("Cargo.lock");
            let cargo_lock_path = cargo_lock_path.exists().then_some(cargo_lock_path);

            // Use block_in_place to allow blocking within async context
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    crate::workspace::get_docs(
                        &crate_name_typed,
                        version,
                        &self.workspace.root,
                        is_workspace_member,
                        cargo_lock_path.as_deref(),
                    )
                    .await
                })
            });

            if let Err(e) = result {
                tracing::error!(crate_name, error = ?e, "Failed to generate docs");
                return Err(LoadError::GenerationFailed {
                    crate_name: crate_name_typed,
                    reason: e.to_string(),
                });
            }
        }

        // Verify the file now exists (generation might have failed silently)
        if !doc_path.exists() {
            return Err(LoadError::NotFoundAt {
                crate_name: crate_name_typed,
                path: doc_path,
            });
        }

        // Load the documentation (either existing or just generated)
        let crate_index = CrateIndex::load(&doc_path).map_err(|e| {
            tracing::error!(crate_name, error = ?e, "Failed to load docs");
            LoadError::ParseFailed {
                crate_name: CrateName::new_unchecked(crate_name),
                reason: e.to_string(),
            }
        })?;

        Ok(self.cache_crate_index(crate_name, crate_index))
    }

    /// Allocate a CrateIndex in the arena and cache it for future lookups.
    fn cache_crate_index(&self, crate_name: &str, crate_index: CrateIndex) -> &CrateIndex {
        let allocated: &CrateIndex = self.arena.alloc(crate_index);
        let arena_ptr = ArenaPtr::new(allocated);
        self.doc_cache
            .borrow_mut()
            .insert(CrateName::new_unchecked(crate_name), arena_ptr);
        allocated
    }

    /// Check if we have the minimum requirements to generate documentation.
    ///
    /// This guards against attempting doc generation in isolated test environments
    /// or read-only filesystems where it would fail.
    fn can_generate_docs(&self, crate_name: &str) -> bool {
        let cargo_toml = self.workspace.root.join("Cargo.toml");

        // Must have Cargo.toml
        if !cargo_toml.exists() {
            tracing::debug!(
                "Cannot generate docs for '{}': no Cargo.toml at {:?}",
                crate_name,
                self.workspace.root
            );
            return false;
        }

        // For workspace members, check that source directory exists
        if self.workspace.members.iter().any(|m| m.matches(crate_name)) {
            let src_dir = self.workspace.root.join("src");
            if !src_dir.exists() {
                tracing::debug!(
                    "Cannot generate docs for '{}': no src/ directory",
                    crate_name
                );
                return false;
            }
        }

        true
    }

    /// Resolve a path like "crate_name::module::Item" to an ItemRef.
    /// Populates suggestions if the path cannot be resolved.
    ///
    /// This method supports cross-crate resolution by discovering crates
    /// from existing JSON files even if they're not in the workspace's known crates.
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

        // Load the crate with discovery (tries normal load, then discovers from JSON files)
        let crate_index = match self.load_crate_with_discovery(crate_name) {
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

    /// Load a crate, discovering it from existing doc files if not in known crates.
    ///
    /// This is useful for loading crates like `serde_core` that are internal
    /// dependencies of `serde` but not directly listed in the workspace's dependencies.
    pub fn load_crate_with_discovery(&self, crate_name: &str) -> Result<&CrateIndex, LoadError> {
        // First try normal loading (checks cache, generates if needed)
        match self.load_crate(crate_name) {
            Ok(index) => return Ok(index),
            Err(LoadError::NotFound { .. }) => {
                // Fall through to discovery
            }
            Err(e) => return Err(e),
        }

        // Discovery: Check if a JSON file exists even though crate isn't in workspace
        let doc_path = CrateName::new_unchecked(crate_name)
            .doc_json_path(&self.workspace.root.join("target/doc"));

        if doc_path.exists() {
            tracing::debug!(
                "Discovered undeclared crate '{}' from existing JSON at {:?}",
                crate_name,
                doc_path
            );

            // Load directly from the JSON file without trying to regenerate
            let crate_index = CrateIndex::load(&doc_path).map_err(|e| LoadError::ParseFailed {
                crate_name: CrateName::new_unchecked(crate_name),
                reason: e.to_string(),
            })?;

            return Ok(self.cache_crate_index(crate_name, crate_index));
        }

        Err(LoadError::NotFound {
            crate_name: CrateName::new_unchecked(crate_name),
        })
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
