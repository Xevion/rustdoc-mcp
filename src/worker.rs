//! Background worker for continuous workspace detection and documentation generation.
//!
//! The worker runs in a loop, detecting workspace changes and pre-generating
//! documentation for crates. Tool handlers can await in-flight generation
//! via shared futures.

use crate::search::CrateIndex;
use crate::stdlib::StdlibDocs;
use crate::tools::set_workspace::handle_set_workspace;
use crate::types::CrateName;
use crate::workspace::{WorkspaceContext, auto_detect_workspace};
use anyhow::Result;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{Duration, Instant, interval_at};

/// Maximum number of parsed CrateIndex entries to keep in memory.
const LRU_CACHE_SIZE: usize = 50;

/// Interval between workspace detection cycles.
const DETECTION_INTERVAL: Duration = Duration::from_secs(5);

/// Type alias for shared doc generation futures.
type SharedDocFuture = Shared<BoxFuture<'static, Result<Arc<CrateIndex>, String>>>;

/// Shared state for documentation caching and generation.
///
/// This is the central coordination point for:
/// - Caching parsed CrateIndex entries (LRU)
/// - Tracking in-flight generation tasks (shared futures)
/// - Storing workspace context
pub struct DocState {
    /// LRU cache of parsed crate indices
    cache: RwLock<LruCache<CrateName, Arc<CrateIndex>>>,

    /// In-flight generation futures (can be awaited by multiple callers)
    in_flight: Mutex<HashMap<CrateName, SharedDocFuture>>,

    /// Current workspace context (if detected/configured)
    workspace: RwLock<Option<WorkspaceContext>>,

    /// Current working directory
    working_directory: RwLock<Option<PathBuf>>,

    /// Path to Cargo.lock (for dependency fingerprinting)
    cargo_lock_path: RwLock<Option<PathBuf>>,

    /// Standard library documentation (if available)
    stdlib: Option<Arc<StdlibDocs>>,
}

impl std::fmt::Debug for DocState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocState")
            .field("cache_size", &self.cache.blocking_read().len())
            .field("in_flight_count", &self.in_flight.blocking_lock().len())
            .field("has_workspace", &self.workspace.blocking_read().is_some())
            .field("has_stdlib", &self.stdlib.is_some())
            .finish()
    }
}

impl DocState {
    /// Create a new DocState with optional stdlib support.
    pub fn new(stdlib: Option<Arc<StdlibDocs>>) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(LRU_CACHE_SIZE).unwrap())),
            in_flight: Mutex::new(HashMap::new()),
            workspace: RwLock::new(None),
            working_directory: RwLock::new(None),
            cargo_lock_path: RwLock::new(None),
            stdlib,
        }
    }

    /// Get the current workspace context.
    pub async fn workspace(&self) -> Option<WorkspaceContext> {
        self.workspace.read().await.clone()
    }

    /// Get the current working directory.
    pub async fn working_directory(&self) -> Option<PathBuf> {
        self.working_directory.read().await.clone()
    }

    /// Get the Cargo.lock path.
    pub async fn cargo_lock_path(&self) -> Option<PathBuf> {
        self.cargo_lock_path.read().await.clone()
    }

    /// Get the stdlib documentation provider.
    pub fn stdlib(&self) -> Option<&Arc<StdlibDocs>> {
        self.stdlib.as_ref()
    }

    /// Check if a workspace has been configured.
    pub async fn has_workspace(&self) -> bool {
        self.workspace.read().await.is_some()
    }

    /// Update the workspace context.
    pub async fn set_workspace(
        &self,
        working_dir: PathBuf,
        workspace: WorkspaceContext,
        cargo_lock: Option<PathBuf>,
    ) {
        *self.working_directory.write().await = Some(working_dir);
        *self.workspace.write().await = Some(workspace);
        *self.cargo_lock_path.write().await = cargo_lock;
    }

    /// Clear cached docs (e.g., when workspace changes).
    pub async fn clear_cache(&self) {
        tracing::debug!("Clearing documentation cache");
        self.cache.write().await.clear();
        self.in_flight.lock().await.clear();
    }

    /// Get docs for a crate, waiting for in-flight generation if needed.
    ///
    /// This is the main entry point for tool handlers. It:
    /// 1. Checks the LRU cache
    /// 2. Checks for in-flight generation (awaits if found)
    /// 3. Starts new generation if needed
    pub async fn get_docs(&self, crate_name: &str) -> Result<Arc<CrateIndex>, String> {
        // Normalize the name so hyphenated lookups (e.g. "rust-stemmers") correctly
        // match entries stored under the normalized key ("rust_stemmers").
        // CrateName::Borrow<str> returns the normalized form, so HashMap hashing is
        // based on the normalized string — we must use the same form for lookups.
        let key = CrateName::new_unchecked(crate_name);

        // 1. Check cache first
        {
            let mut cache = self.cache.write().await;
            if let Some(index) = cache.get(&key) {
                tracing::debug!(crate_name, "Cache hit");
                return Ok(index.clone());
            }
        }

        // 2. Check for in-flight generation
        let maybe_future = {
            let in_flight = self.in_flight.lock().await;
            in_flight.get(&key).cloned()
        };

        if let Some(future) = maybe_future {
            tracing::debug!(crate_name, "Awaiting in-flight generation");
            return future.await;
        }

        // 3. Start new generation
        self.generate_docs(crate_name).await
    }

    /// Start documentation generation for a crate.
    ///
    /// Creates a shared future that can be awaited by multiple callers.
    async fn generate_docs(&self, crate_name: &str) -> Result<Arc<CrateIndex>, String> {
        let workspace = self
            .workspace
            .read()
            .await
            .clone()
            .ok_or_else(|| "No workspace configured".to_string())?;

        let working_dir = self
            .working_directory
            .read()
            .await
            .clone()
            .ok_or_else(|| "No working directory configured".to_string())?;

        let cargo_lock = self.cargo_lock_path.read().await.clone();

        // Get crate metadata
        let meta = workspace
            .get_crate(crate_name)
            .ok_or_else(|| format!("Crate '{}' not found in workspace", crate_name))?;

        let is_workspace_member = meta.origin == crate::workspace::CrateOrigin::Local;
        let version = meta.version.clone();
        let crate_name_owned = CrateName::new_unchecked(crate_name);

        // Create the generation future
        let generation_future: BoxFuture<'static, Result<Arc<CrateIndex>, String>> =
            Box::pin(async move {
                crate::workspace::get_docs(
                    &crate_name_owned,
                    version.as_deref(),
                    &working_dir,
                    is_workspace_member,
                    cargo_lock.as_deref(),
                )
                .await
                .map(Arc::new)
                .map_err(|e| e.to_string())
            });

        // Make it shared so multiple callers can await
        let shared_future = generation_future.shared();

        let key = CrateName::new_unchecked(crate_name);

        // Store in in_flight map
        {
            let mut in_flight = self.in_flight.lock().await;
            in_flight.insert(key.clone(), shared_future.clone());
        }

        tracing::info!(crate_name, "Starting documentation generation");

        // Await the result
        let result = shared_future.await;

        // Remove from in_flight. Must use the normalized CrateName key — removing with
        // a raw &str containing hyphens (e.g. "rust-stemmers") would hash differently
        // from the normalized key ("rust_stemmers") and silently miss.
        {
            let mut in_flight = self.in_flight.lock().await;
            if in_flight.remove(&key).is_none() {
                tracing::warn!(
                    crate_name,
                    normalized = key.normalized(),
                    "in_flight entry was missing during removal — possible concurrent generation"
                );
            }
        }

        // Cache on success
        if let Ok(ref index) = result {
            let mut cache = self.cache.write().await;
            cache.put(key, index.clone());
            tracing::debug!(crate_name, "Docs cached in memory");
        } else if let Err(ref e) = result {
            tracing::warn!(crate_name, error = %e, "Documentation generation failed");
        }

        result
    }

    /// Check if docs are cached for a crate.
    ///
    /// Uses the normalized crate name for the lookup so that hyphenated names
    /// (e.g. "rust-stemmers") correctly hit entries stored under "rust_stemmers".
    pub async fn is_cached(&self, crate_name: &str) -> bool {
        let key = CrateName::new_unchecked(crate_name);
        self.cache.read().await.contains(&key)
    }

    /// Check if generation is in progress for a crate.
    ///
    /// Uses the normalized crate name so hyphenated lookups match correctly.
    pub async fn is_generating(&self, crate_name: &str) -> bool {
        let key = CrateName::new_unchecked(crate_name);
        self.in_flight.lock().await.contains_key(&key)
    }

    /// Get a cached CrateIndex without triggering generation.
    pub async fn get_cached(&self, crate_name: &str) -> Option<Arc<CrateIndex>> {
        let key = CrateName::new_unchecked(crate_name);
        self.cache.write().await.get(&key).cloned()
    }

    /// Put a CrateIndex directly into the cache.
    pub async fn put_cached(&self, crate_name: CrateName, index: Arc<CrateIndex>) {
        self.cache.write().await.put(crate_name, index);
    }
}

/// Background worker that continuously detects workspaces and pre-generates docs.
pub struct BackgroundWorker {
    state: Arc<DocState>,
}

impl BackgroundWorker {
    /// Create a new background worker.
    pub fn new(state: Arc<DocState>) -> Self {
        Self { state }
    }

    /// Run the background worker loop.
    ///
    /// This runs indefinitely, performing:
    /// 1. Workspace detection (every 5 seconds)
    /// 2. Documentation pre-generation for discovered crates
    pub async fn run(&self) {
        // Run detection immediately on start, before the periodic loop begins.
        self.detect_and_generate().await;

        // Use interval_at so the first tick fires DETECTION_INTERVAL after now,
        // not immediately. tokio::interval() fires its first tick at T=0, which
        // would cause a redundant detection right after the initial call above.
        let mut ticker = interval_at(Instant::now() + DETECTION_INTERVAL, DETECTION_INTERVAL);

        loop {
            ticker.tick().await;
            self.detect_and_generate().await;
        }
    }

    /// Perform one cycle of workspace detection and doc generation.
    async fn detect_and_generate(&self) {
        // 1. Detect workspace
        let Some(workspace_path) = auto_detect_workspace().await else {
            tracing::trace!("No workspace detected");
            return;
        };

        // 2. Check if workspace changed
        let current_workspace = self.state.workspace().await;
        let workspace_changed = current_workspace
            .as_ref()
            .map(|w| w.root != workspace_path)
            .unwrap_or(true);

        if !workspace_changed {
            // Workspace unchanged — only generate docs for crates not yet cached
            tracing::debug!(workspace = %workspace_path.display(), "Workspace unchanged, scanning for uncached crates");
            if let Some(workspace) = current_workspace {
                self.generate_uncached_docs(&workspace).await;
            }
            return;
        }

        // 3. Configure the new workspace
        tracing::info!(workspace_path = %workspace_path.display(), "Workspace change detected, reconfiguring");

        match handle_set_workspace(workspace_path.display().to_string(), None).await {
            Ok((canonical_path, workspace_info, _changed)) => {
                // Update state
                let cargo_lock = canonical_path.join("Cargo.lock");
                let cargo_lock = if cargo_lock.exists() {
                    Some(cargo_lock)
                } else {
                    None
                };

                // Clear old cache when workspace changes
                self.state.clear_cache().await;

                self.state
                    .set_workspace(canonical_path.clone(), workspace_info.clone(), cargo_lock)
                    .await;

                tracing::info!(
                    workspace = %canonical_path.display(),
                    members = workspace_info.members.len(),
                    crates = workspace_info.crate_info.len(),
                    "Background worker configured workspace"
                );

                // 4. Start generating docs
                self.generate_uncached_docs(&workspace_info).await;
            }
            Err(e) => {
                tracing::warn!(error = ?e, "Background workspace detection failed");
            }
        }
    }

    /// Generate docs for crates that aren't cached yet.
    async fn generate_uncached_docs(&self, workspace: &WorkspaceContext) {
        let prioritized = workspace.prioritized_crates();
        let total = prioritized.len();

        // Pre-scan to build a summary for the log line before doing any work.
        let mut already_cached: u32 = 0;
        let mut already_generating: u32 = 0;
        let mut to_generate: Vec<CrateName> = Vec::new();

        for crate_name in &prioritized {
            if StdlibDocs::is_stdlib_crate(crate_name.as_str()) {
                continue;
            }
            if self.state.is_cached(crate_name.as_str()).await {
                already_cached += 1;
            } else if self.state.is_generating(crate_name.as_str()).await {
                already_generating += 1;
            } else {
                to_generate.push(crate_name.clone());
            }
        }

        tracing::debug!(
            total,
            cached = already_cached,
            in_flight = already_generating,
            pending = to_generate.len(),
            "Documentation generation scan"
        );

        for crate_name in to_generate {
            // Generate docs (this will cache on success)
            match self.state.get_docs(crate_name.as_str()).await {
                Ok(_) => {
                    tracing::info!(crate_name = %crate_name, "Background documentation ready");
                }
                Err(e) => {
                    tracing::warn!(crate_name = %crate_name, error = %e, "Background doc generation failed");
                }
            }

            // Yield to allow other tasks to run
            tokio::task::yield_now().await;
        }
    }
}

/// Spawn the background worker as a tokio task.
///
/// Returns a handle to the spawned task.
pub fn spawn_background_worker(state: Arc<DocState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker = BackgroundWorker::new(state);

        // Run with panic recovery
        loop {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // We need to create a new runtime context here for the panic boundary
            }));

            if result.is_err() {
                tracing::error!("Background worker panicked, restarting in 5 seconds");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }

            worker.run().await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_doc_state_new() {
        let state = DocState::new(None);
        assert!(!state.has_workspace().await);
        assert!(state.workspace().await.is_none());
        assert!(state.working_directory().await.is_none());
    }

    #[tokio::test]
    async fn test_cache_operations() {
        let state = DocState::new(None);

        // Should not be cached initially
        assert!(!state.is_cached("test_crate").await);
        assert!(state.get_cached("test_crate").await.is_none());
    }
}
