//! QueryContext provides request-scoped context for documentation queries with automatic caching.
//! Also includes path parsing utilities for resolving item queries.

use crate::error::LoadError;
use crate::item::ItemRef;
use crate::search::index::source_digest;
use crate::search::rustdoc::CrateIndex;
use crate::types::CrateName;
use crate::workspace::WorkspaceContext;
use rapidfuzz::distance::jaro_winkler;
use rustdoc_types::{Id, Item, ItemEnum};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
    path::{Path, PathBuf},
    sync::Arc,
};

/// A pre-loaded crate index injected into a [`QueryContext`] before any lookups.
///
/// Used for cases where the source rustdoc JSON lives outside the workspace
/// `target/doc/` directory (e.g., stdlib docs in the nightly sysroot). The
/// context short-circuits its normal lookup for any crate registered here.
#[derive(Clone)]
pub struct PreloadedCrate {
    /// Already-loaded crate index. The `Arc` keeps the data alive for the
    /// entire `QueryContext` lifetime.
    pub index: Arc<CrateIndex>,
    /// Absolute path to the source rustdoc JSON file. Used for mtime-based
    /// cache invalidation in [`crate::search::TermIndex`].
    pub source_path: PathBuf,
    /// Absolute path where the compiled search index should be stored.
    /// Distinct from `source_path` so the cache can live in a user-writable
    /// directory even when the source lives in a read-only toolchain install.
    pub index_cache_path: PathBuf,
}

impl Debug for PreloadedCrate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Intentionally omit `index` — CrateIndex holds tens of MBs of
        // rustdoc data and has no Debug impl (nor should it).
        f.debug_struct("PreloadedCrate")
            .field("source_path", &self.source_path)
            .field("index_cache_path", &self.index_cache_path)
            .finish_non_exhaustive()
    }
}

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
/// - `Vec` → `path_components = [Vec]`
/// - `std::vec::Vec` → `path_components = [std, vec, Vec]`
/// - `collections::HashMap` → `path_components = [collections, HashMap]`
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
            path_components: vec![String::new()],
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

/// Represents a single query context with its own cache and state.
/// Automatically cleans up when dropped.
pub struct QueryContext {
    workspace: Arc<WorkspaceContext>,
    /// Per-query cache of loaded documentation indices.
    ///
    /// `Arc`, not an arena: arena allocation never runs `CrateIndex`'s destructor, so
    /// every parsed crate would leak. It also keeps addresses stable across rehashes.
    ///
    /// # Invariant
    ///
    /// Entries are only inserted, never removed; a borrow of one would dangle.
    doc_cache: RefCell<HashMap<CrateName, Arc<CrateIndex>>>,
    /// Process-wide parsed-crate cache, shared with the background worker.
    ///
    /// Without it each request re-parses every crate's rustdoc JSON into its own
    /// map, duplicating what the worker already did.
    shared: Option<Arc<crate::worker::DocState>>,
    /// Negative cache: crate names for which doc generation already failed this session.
    /// Prevents retrying expensive cargo rustdoc invocations for the same crate.
    failed_crates: RefCell<std::collections::HashSet<String>>,
    /// Pre-loaded crate indices registered at construction time (e.g., stdlib).
    ///
    /// Plain `HashMap` (not `RefCell`) because preloading is a construction-time
    /// decision — we never mutate it after `QueryContext` is built. This lets
    /// [`Self::load_crate`] return `&CrateIndex` bound to `&self` via a plain
    /// `Arc::deref`, with no `unsafe` required.
    preloaded: HashMap<CrateName, PreloadedCrate>,
}

impl Debug for QueryContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryContext")
            .field("workspace", &self.workspace.root)
            .field("doc_cache_len", &self.doc_cache.borrow().len())
            .field("failed_crates_len", &self.failed_crates.borrow().len())
            .field("preloaded_len", &self.preloaded.len())
            .finish_non_exhaustive()
    }
}

impl QueryContext {
    /// Create a new query context for the given workspace with no preloaded crates.
    pub fn new(workspace: Arc<WorkspaceContext>) -> Self {
        Self::with_preloaded(workspace, HashMap::new())
    }

    /// Create a new query context with a set of pre-loaded crate indices.
    ///
    /// Use this when some crates should bypass the workspace `target/doc/`
    /// lookup — most notably stdlib crates, whose rustdoc JSON lives in the
    /// nightly sysroot and whose search index must be cached in a
    /// user-writable directory rather than alongside the source JSON.
    ///
    /// # Invariant
    ///
    /// Keys in `preloaded` **must** already be in normalized form (use
    /// underscores, not hyphens). Lookup sites resolve by `&str`, and
    /// [`CrateName`]'s `Borrow<str>` impl exposes the normalized form, so a
    /// key inserted as `"serde-json"` would be unreachable by any `&str`
    /// lookup. A debug assertion enforces this at construction.
    pub fn with_preloaded(
        workspace: Arc<WorkspaceContext>,
        preloaded: HashMap<CrateName, PreloadedCrate>,
    ) -> Self {
        debug_assert!(
            preloaded
                .keys()
                .all(|k| k.as_str() == k.normalized() || !k.as_str().contains('-')),
            "preloaded crate keys must be normalized (no hyphens in original form); \
             a hyphenated key would be unreachable via &str lookup"
        );
        Self {
            workspace,
            doc_cache: RefCell::new(HashMap::new()),
            failed_crates: RefCell::new(std::collections::HashSet::new()),
            preloaded,
            shared: None,
        }
    }

    /// Create a context that reuses parsed crates from the shared cache.
    pub fn with_shared_cache(
        workspace: Arc<WorkspaceContext>,
        shared: Arc<crate::worker::DocState>,
    ) -> Self {
        Self {
            workspace,
            doc_cache: RefCell::new(HashMap::new()),
            failed_crates: RefCell::new(std::collections::HashSet::new()),
            preloaded: HashMap::new(),
            shared: Some(shared),
        }
    }

    /// Resolve the source rustdoc JSON path for a crate, respecting preloaded entries.
    ///
    /// For preloaded crates (e.g., stdlib), returns the path stored in the
    /// [`PreloadedCrate`] — typically a sysroot path. For all other crates,
    /// returns the standard `<workspace>/target/doc/<crate>.json` location.
    pub fn doc_source_path(&self, crate_name: &str) -> PathBuf {
        if let Some(pre) = self.preloaded.get(crate_name) {
            return pre.source_path.clone();
        }
        CrateName::new_unchecked(crate_name).doc_json_path(&self.workspace.root.join("target/doc"))
    }

    /// Resolve the compiled search-index cache path for a crate, respecting preloaded entries.
    ///
    /// For preloaded crates, returns the explicit cache path — typically under
    /// a user-writable directory like `$XDG_CACHE_HOME/rustdoc-mcp/...`. For
    /// workspace crates, the cache sits alongside the source JSON in `target/doc/`.
    pub fn index_cache_path(&self, crate_name: &str) -> PathBuf {
        if let Some(pre) = self.preloaded.get(crate_name) {
            return pre.index_cache_path.clone();
        }
        CrateName::new_unchecked(crate_name).index_path(&self.workspace.root.join("target/doc"))
    }

    /// The process-wide cache backing this context, if it has one.
    pub(crate) const fn shared(&self) -> Option<&Arc<crate::worker::DocState>> {
        self.shared.as_ref()
    }

    /// Digest of a crate's rustdoc JSON, read fresh at each call.
    ///
    /// Deliberately not memoized: the worker can regenerate a crate's JSON while a
    /// request is in flight, and a digest captured beforehand would validate the
    /// index built from the superseded content.
    pub(crate) fn source_digest_of(&self, crate_name: &str) -> Option<u64> {
        source_digest(&self.doc_source_path(crate_name))
    }

    /// Returns true if documentation generation for this crate failed earlier in this
    /// query context's lifetime. Used to skip redundant retry attempts.
    pub fn is_generation_failed(&self, crate_name: &str) -> bool {
        self.failed_crates.borrow().contains(crate_name)
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
        // Preloaded crates (e.g., stdlib) bypass the workspace target/doc/ lookup.
        // The Arc keeps the CrateIndex alive for the entire QueryContext lifetime,
        // and `self.preloaded` is never mutated after construction, so the returned
        // reference is valid for `&self`.
        if let Some(pre) = self.preloaded.get(crate_name) {
            return Ok(pre.index.as_ref());
        }

        // Check cache first and return reference with proper lifetime
        if let Some(cached) = self.doc_cache.borrow().get(crate_name) {
            let ptr: *const CrateIndex = Arc::as_ptr(cached);
            // SAFETY: doc_cache owns this Arc and never drops it, so it outlives self.
            return Ok(unsafe { &*ptr });
        }

        // Reuse the worker's parse when it has one, rather than building a second
        // copy of the same map.
        if let Some(index) = self.shared_lookup(crate_name) {
            return Ok(self.cache_crate_arc(crate_name, index));
        }

        // Note: stdlib handlers construct a sentinel workspace root at "/".
        // Cross-crate references between stdlib crates (e.g. `std` referencing
        // items from `core` or `alloc_crate`) legitimately fall through to
        // this cold path and return `NotFound`. That's fine — those lookups
        // are opportunistic and callers handle the failure.

        // Try to find and load the JSON doc file
        let doc_path = self.doc_source_path(crate_name);
        let crate_name_typed = CrateName::new_unchecked(crate_name);

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
                self.failed_crates
                    .borrow_mut()
                    .insert(crate_name.to_string());
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

    /// Read a parsed crate out of the shared cache, if one matches the current source.
    ///
    /// An entry parsed from a JSON that has since been regenerated is stale, and
    /// serving it would hand back documentation for code that no longer exists.
    fn shared_lookup(&self, crate_name: &str) -> Option<Arc<CrateIndex>> {
        let shared = self.shared.as_ref()?;
        let cached = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(shared.get_cached(crate_name))
        })?;

        // An unreadable source cannot contradict the cache, so keep it.
        let Some(on_disk) = self.source_digest_of(crate_name) else {
            return Some(cached);
        };

        if cached.source_digest() == on_disk {
            return Some(cached);
        }

        tracing::debug!(crate_name, "Shared cache entry is stale, reparsing");
        None
    }

    /// Cache a `CrateIndex` for the rest of this context's lifetime.
    ///
    /// Also publishes to the shared cache so the next request reuses this parse.
    fn cache_crate_index(&self, crate_name: &str, crate_index: CrateIndex) -> &CrateIndex {
        let cached = Arc::new(crate_index);

        if let Some(shared) = &self.shared {
            let key = CrateName::new_unchecked(crate_name);
            let value = Arc::clone(&cached);
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(shared.put_cached(key, value));
            });
        }

        self.cache_crate_arc(crate_name, cached)
    }

    /// Hold an already-parsed crate for the rest of this context's lifetime.
    fn cache_crate_arc(&self, crate_name: &str, cached: Arc<CrateIndex>) -> &CrateIndex {
        let ptr: *const CrateIndex = Arc::as_ptr(&cached);
        self.doc_cache
            .borrow_mut()
            .insert(CrateName::new_unchecked(crate_name), cached);
        // SAFETY: doc_cache now owns this Arc and never drops it before self.
        unsafe { &*ptr }
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
        let Ok(crate_index) = self.load_crate_with_discovery(crate_name) else {
            // Generate suggestions for available crates
            suggestions.extend(
                self.workspace
                    .members
                    .iter()
                    .map(super::super::types::CrateName::as_str)
                    .chain(self.workspace.dependency_names())
                    .map(|name| PathSuggestion {
                        path: name.to_string(),
                        item: None,
                        score: jaro_winkler::similarity(crate_name.chars(), name.chars()),
                    }),
            );
            return None;
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

    /// Resolve a path against where items are defined, rather than how they are exported.
    ///
    /// rustdoc omits a non-public module's contents from its parent's item list, so the
    /// module walk in [`Self::resolve_path`] cannot reach an item whose path runs through
    /// one, even though the tool prints exactly that path for the item it did resolve.
    /// The crate's path table records definition sites and does reach it.
    ///
    /// Scoped to explicit user path queries. Re-export resolution during indexing keeps
    /// using the module walk, which only ever surfaces publicly reachable items.
    pub fn resolve_definition_path<'a>(
        &'a self,
        path: &str,
        kind: Option<crate::search::ItemKind>,
    ) -> Option<ItemRef<'a, Item>> {
        self.resolve_definition_path_inner(path, kind, true)
    }

    fn resolve_definition_path_inner<'a>(
        &'a self,
        path: &str,
        kind: Option<crate::search::ItemKind>,
        follow_foreign: bool,
    ) -> Option<ItemRef<'a, Item>> {
        let (crate_name, _) = path
            .find("::")
            .map_or((path, None), |i| (&path[..i], Some(i + 2)));
        let crate_index = self.load_crate_with_discovery(crate_name).ok()?;

        // The crate segment reaches here in whatever form the user typed, while the
        // path table always spells it with underscores.
        let wanted: Vec<&str> = path.split("::").collect();
        let mut matched: Option<rustdoc_types::Id> = None;
        for (id, summary) in crate_index.paths() {
            if summary.path.len() != wanted.len() {
                continue;
            }
            let same =
                summary
                    .path
                    .iter()
                    .zip(&wanted)
                    .enumerate()
                    .all(|(position, (segment, want))| {
                        if position == 0 {
                            *segment == CrateName::normalize(want)
                        } else {
                            segment == want
                        }
                    });
            if !same {
                continue;
            }
            // Several items can share one path across Rust's namespaces, so a kind
            // hint is the only thing that separates them. Without one, an ambiguous
            // path resolves to nothing rather than to an arbitrary winner.
            let qualifies = match kind {
                Some(wanted) => crate_index
                    .get_item(*id)
                    .is_some_and(|item| crate::search::matches_kind(&item.inner, wanted)),
                None => true,
            };
            if !qualifies {
                continue;
            }
            if matched.is_some() {
                return None;
            }
            matched = Some(*id);
        }

        if let Some(id) = matched {
            return self.get_item(crate_index, id);
        }
        if !follow_foreign {
            return None;
        }

        // The crate names the item but another crate defines it. A path table records
        // such an item under its defining path, tagged with the crate that owns it,
        // which is the only trace std leaves of what it re-exports from alloc.
        let tail = *wanted.last()?;
        let mut foreign: Option<String> = None;
        for summary in crate_index.paths().values() {
            if summary.crate_id == 0 || summary.path.last().map(String::as_str) != Some(tail) {
                continue;
            }
            if !is_subsequence(&wanted[1..], &summary.path) {
                continue;
            }
            let Some(defining) = crate_index.external_crate_name(summary.crate_id) else {
                continue;
            };
            let candidate = format!("{defining}::{}", summary.path[1..].join("::"));
            if foreign.as_ref().is_some_and(|found| *found != candidate) {
                return None;
            }
            foreign = Some(candidate);
        }

        self.resolve_definition_path_inner(&foreign?, kind, false)
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

        // Discovery: Check if a JSON file exists even though crate isn't in workspace.
        // Route through doc_source_path so any future path-resolution changes
        // (including preloaded crates) are honored consistently.
        let doc_path = self.doc_source_path(crate_name);

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
    #[allow(clippy::self_only_used_in_recursion)]
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
        let segment_end = remaining.find("::").map_or(path.len(), |x| index + x);
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
        suggestions.extend(Self::generate_suggestions(item, path, index));
        None
    }

    /// Generate fuzzy suggestions for items that are similar to the query.
    fn generate_suggestions<'a>(
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
        id: Id,
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
            item = item.get(Id(*id))?;

            // Handle re-exports
            if let ItemEnum::Use(use_item) = item.inner() {
                if let Some(target_id) = use_item.id {
                    // A re-export can name a target in another crate, whose own
                    // module chain may be non-public there. std pulling Vec and
                    // BTreeMap out of alloc is the common case.
                    item = item
                        .get(target_id)
                        .or_else(|| self.resolve_path(&use_item.source, &mut vec![]))
                        .or_else(|| self.resolve_definition_path(&use_item.source, None))?;
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

/// Whether every segment of `needle` appears in `haystack`, in order.
///
/// A re-export names an item by a shorter path than the one defining it, so
/// `collections::BTreeMap` must be recognised inside `collections::btree::map::BTreeMap`.
fn is_subsequence(needle: &[&str], haystack: &[String]) -> bool {
    let mut segments = haystack.iter();
    needle
        .iter()
        .all(|want| segments.any(|segment| segment == want))
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
    pub const fn item(&self) -> Option<ItemRef<'a, Item>> {
        self.item
    }

    /// Get the relevance score (0.0 to 1.0, higher is better).
    pub const fn score(&self) -> f64 {
        self.score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use rustdoc_types::{Crate, Target};
    use std::collections::HashMap as StdHashMap;

    fn empty_crate() -> Crate {
        Crate {
            root: Id(0),
            crate_version: None,
            includes_private: false,
            index: StdHashMap::new(),
            paths: StdHashMap::new(),
            external_crates: StdHashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_string(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        }
    }

    fn context(root: &Path) -> QueryContext {
        QueryContext::new(Arc::new(WorkspaceContext {
            root: root.to_path_buf(),
            members: Vec::new(),
            crate_info: StdHashMap::new(),
            root_crate: None,
        }))
    }

    /// The worker and the request path each used to parse the same JSON into their
    /// own map. A context wired to the shared cache must reuse that parse: here the
    /// crate exists only in the cache, with no JSON on disk to fall back to.
    #[tokio::test(flavor = "multi_thread")]
    async fn load_crate_reuses_the_shared_cache_instead_of_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = Arc::new(crate::worker::DocState::new(None));
        let cached = Arc::new(CrateIndex::from_crate(empty_crate()));

        state
            .put_cached(CrateName::new_unchecked("demo"), Arc::clone(&cached))
            .await;

        let ctx = QueryContext::with_shared_cache(
            Arc::new(WorkspaceContext {
                root: dir.path().to_path_buf(),
                members: Vec::new(),
                crate_info: StdHashMap::new(),
                root_crate: None,
            }),
            Arc::clone(&state),
        );

        let loaded = ctx.load_crate("demo");

        check!(loaded.is_ok());
        check!(std::ptr::eq(
            loaded.expect("cached crate"),
            Arc::as_ptr(&cached)
        ));
    }

    /// The shared cache holds parses from earlier requests. If the JSON has been
    /// regenerated since, reusing that parse serves stale documentation, so a cached
    /// entry whose digest no longer matches the file on disk must be ignored.
    #[tokio::test(flavor = "multi_thread")]
    async fn shared_cache_entry_is_ignored_when_source_changed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let doc_dir = dir.path().join("target/doc");
        std::fs::create_dir_all(&doc_dir).expect("doc dir");

        // Real JSON on disk, and a cached parse that did not come from it.
        let source = doc_dir.join("demo.json");
        std::fs::write(
            &source,
            serde_json::to_string(&empty_crate()).expect("serialize"),
        )
        .expect("write source");

        let state = Arc::new(crate::worker::DocState::new(None));
        state
            .put_cached(
                CrateName::new_unchecked("demo"),
                Arc::new(CrateIndex::from_crate(empty_crate())),
            )
            .await;

        let ctx = QueryContext::with_shared_cache(
            Arc::new(WorkspaceContext {
                root: dir.path().to_path_buf(),
                members: Vec::new(),
                crate_info: StdHashMap::new(),
                root_crate: None,
            }),
            Arc::clone(&state),
        );

        let loaded = ctx.load_crate("demo").expect("loads from disk");

        check!(loaded.source_digest() == source_digest_of(&source));
    }

    fn source_digest_of(path: &Path) -> u64 {
        xxhash_rust::xxh3::xxh3_64(&std::fs::read(path).expect("read source"))
    }

    /// A cached `CrateIndex` owns a parsed rustdoc JSON; outliving its context leaks it.
    #[test]
    fn dropping_query_context_releases_cached_crate_indices() {
        let dir = tempfile::tempdir().expect("tempdir");

        let weak = {
            let ctx = context(dir.path());
            ctx.cache_crate_index("demo", CrateIndex::from_crate(empty_crate()));
            let cache = ctx.doc_cache.borrow();
            Arc::downgrade(cache.get("demo").expect("just cached"))
        };

        check!(weak.upgrade().is_none());
    }
}
