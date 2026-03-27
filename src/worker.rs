//! Background worker for continuous workspace detection and documentation generation.
//!
//! The worker runs in a loop, detecting workspace changes and pre-generating
//! documentation for crates. Tool handlers can await in-flight generation
//! via shared futures. Supports graceful shutdown via `CancellationToken`.

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
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Maximum number of parsed CrateIndex entries to keep in memory.
const LRU_CACHE_SIZE: usize = 50;

/// Interval between workspace detection cycles.
const DETECTION_INTERVAL: Duration = Duration::from_secs(5);

/// Timeout for graceful shutdown before forcefully terminating.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Type alias for shared doc generation futures.
type SharedDocFuture = Shared<BoxFuture<'static, Result<Arc<CrateIndex>, String>>>;

/// Cancellation-aware helpers for background tasks.
///
/// Wraps a `CancellationToken` and `TaskTracker` to provide structured
/// concurrency with cancellation-aware tick and sleep operations.
#[derive(Clone)]
pub struct ServiceContext {
    token: CancellationToken,
    tracker: TaskTracker,
}

impl ServiceContext {
    /// Create a new service context.
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tracker: TaskTracker::new(),
        }
    }

    /// Get the cancellation token.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Wait for the next interval tick, returning `false` if cancelled.
    pub async fn tick(&self, interval: &mut tokio::time::Interval) -> bool {
        tokio::select! {
            _ = interval.tick() => true,
            () = self.token.cancelled() => false,
        }
    }

    /// Initiate graceful shutdown: cancel all tasks and wait for completion.
    pub async fn shutdown(self) -> bool {
        self.token.cancel();
        self.tracker.close();
        tokio::select! {
            () = self.tracker.wait() => {
                tracing::info!("All background tasks completed");
                true
            }
            () = tokio::time::sleep(SHUTDOWN_TIMEOUT) => {
                tracing::warn!("Shutdown timed out after {}s", SHUTDOWN_TIMEOUT.as_secs());
                false
            }
        }
    }

    /// Check if shutdown has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

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

        // Remove from in_flight
        {
            let mut in_flight = self.in_flight.lock().await;
            if in_flight.remove(&key).is_none() {
                tracing::warn!(
                    crate_name,
                    normalized = key.normalized(),
                    "in_flight entry was missing during removal"
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
    pub async fn is_cached(&self, crate_name: &str) -> bool {
        let key = CrateName::new_unchecked(crate_name);
        self.cache.read().await.contains(&key)
    }

    /// Check if generation is in progress for a crate.
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
struct BackgroundWorker {
    state: Arc<DocState>,
    ctx: ServiceContext,
}

impl BackgroundWorker {
    fn new(state: Arc<DocState>, ctx: ServiceContext) -> Self {
        Self { state, ctx }
    }

    /// Run the background worker loop until cancelled.
    async fn run(&self) {
        // Run detection immediately on start
        self.detect_and_generate().await;

        let mut ticker = interval_at(Instant::now() + DETECTION_INTERVAL, DETECTION_INTERVAL);

        while self.ctx.tick(&mut ticker).await {
            self.detect_and_generate().await;
        }

        tracing::info!("Background worker shutting down");
    }

    /// Perform one cycle of workspace detection and doc generation.
    async fn detect_and_generate(&self) {
        if self.ctx.is_cancelled() {
            return;
        }

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
                let cargo_lock = canonical_path.join("Cargo.lock");
                let cargo_lock = if cargo_lock.exists() {
                    Some(cargo_lock)
                } else {
                    None
                };

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
            // Check cancellation between crate generations
            if self.ctx.is_cancelled() {
                tracing::debug!("Stopping doc generation due to shutdown");
                return;
            }

            match self.state.get_docs(crate_name.as_str()).await {
                Ok(_) => {
                    tracing::info!(crate_name = %crate_name, "Background documentation ready");
                }
                Err(e) => {
                    tracing::warn!(crate_name = %crate_name, error = %e, "Background doc generation failed");
                }
            }

            tokio::task::yield_now().await;
        }
    }
}

/// Spawn the background worker as a tracked task.
///
/// Returns the `ServiceContext` which can be used to trigger graceful shutdown.
pub fn spawn_background_worker(state: Arc<DocState>) -> ServiceContext {
    let ctx = ServiceContext::new();
    let worker = BackgroundWorker::new(state, ctx.clone());

    ctx.tracker.spawn(async move {
        worker.run().await;
    });

    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    #[tokio::test]
    async fn test_doc_state_new() {
        let state = DocState::new(None);
        check!(!state.has_workspace().await);
        check!(state.workspace().await.is_none());
        check!(state.working_directory().await.is_none());
    }

    #[tokio::test]
    async fn test_cache_operations() {
        let state = DocState::new(None);
        check!(!state.is_cached("test_crate").await);
        check!(state.get_cached("test_crate").await.is_none());
    }

    #[tokio::test]
    async fn test_service_context_cancellation() {
        let ctx = ServiceContext::new();
        check!(!ctx.is_cancelled());

        ctx.token().cancel();
        check!(ctx.is_cancelled());
    }

    #[tokio::test]
    async fn test_service_context_tick_cancelled() {
        let ctx = ServiceContext::new();
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        interval.tick().await; // consume first immediate tick

        // Cancel before tick
        ctx.token().cancel();
        let result = ctx.tick(&mut interval).await;
        check!(!result);
    }

    #[tokio::test]
    async fn test_shutdown_completes() {
        let ctx = ServiceContext::new();
        let completed = ctx.shutdown().await;
        check!(completed);
    }
}
