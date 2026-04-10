//! Standard library documentation discovery and loading.
//!
//! Provides access to pre-generated rustdoc JSON for std, core, alloc, and other
//! standard library crates from the `rust-docs-json` nightly component.

use crate::search::{CrateIndex, PreloadedCrate, QueryContext};
use crate::types::CrateName;
use crate::workspace::{CrateMetadata, CrateOrigin, WorkspaceContext};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Standard library crates available when `rust-docs-json` component is installed.
pub const STDLIB_CRATES: &[&str] = &["std", "core", "alloc", "proc_macro", "test"];

/// Manages access to standard library documentation.
///
/// Discovers pre-generated rustdoc JSON from the nightly toolchain's
/// `rust-docs-json` component and provides lazy loading of crate indices.
pub struct StdlibDocs {
    /// Path to the nightly sysroot
    sysroot: PathBuf,
    /// Lazily loaded crate indices
    loaded: RwLock<HashMap<CrateName, Arc<CrateIndex>>>,
    /// Rustc version (for display purposes)
    rustc_version: String,
    /// Root directory for compiled search-index caches.
    ///
    /// Defaults to `<dirs::cache_dir()>/rustdoc-mcp` when discovered normally;
    /// tests should override via [`Self::with_cache_root`] to point at a
    /// `TempDir` for isolation.
    cache_root: PathBuf,
}

impl std::fmt::Debug for StdlibDocs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdlibDocs")
            .field("sysroot", &self.sysroot)
            .field("rustc_version", &self.rustc_version)
            .field("cache_root", &self.cache_root)
            .field("loaded_count", &self.loaded.blocking_read().len())
            .finish()
    }
}

/// Default cache root: `<dirs::cache_dir()>/rustdoc-mcp`, falling back to
/// `std::env::temp_dir().join("rustdoc-mcp")` when the platform has no known
/// cache location. Never falls back to the current working directory — that
/// would pollute user project directories.
fn default_cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rustdoc-mcp")
}

/// Maximum length for the version-token prefix in a cache directory name.
///
/// Caps the on-disk path length to stay well under Windows `MAX_PATH` (260)
/// when combined with a long `LOCALAPPDATA` cache root and the `stdlib/<crate>.index`
/// suffix. Also bounds the attack surface if rustc ever emits an unreasonably
/// long version token.
const MAX_VERSION_TOKEN_LEN: usize = 64;

/// Build a human-readable, collision-safe cache directory name from the
/// `rustc --version` output.
///
/// Combines the semver+channel token (e.g., `1.88.0-nightly`) with the first
/// 8 hex characters of an `xxh3_64` hash of the full version string. This
/// disambiguates two nightlies that share a nominal version but have different
/// commit hashes, while keeping the directory name inspectable at a glance.
///
/// Falls back to a pure 8-char hash if the version token is malformed,
/// empty, oversized, or otherwise fails the path-safety checks — a defensive
/// guard against future rustc output format changes.
fn cache_dir_name(rustc_version: &str) -> String {
    use xxhash_rust::xxh3::xxh3_64;
    let hash = xxh3_64(rustc_version.as_bytes());
    let short_hash = format!("{hash:016x}");
    let short_hash = &short_hash[..8];

    let is_path_safe = |ver: &str| {
        !ver.is_empty()
            && ver.len() <= MAX_VERSION_TOKEN_LEN
            && !ver.starts_with('.')
            && !ver.starts_with('-')
            && !ver.contains("..")
            && ver
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    };

    match rustc_version.split_whitespace().nth(1) {
        Some(ver) if is_path_safe(ver) => format!("{ver}-{short_hash}"),
        _ => short_hash.to_string(),
    }
}

impl StdlibDocs {
    /// Discover the nightly sysroot and verify `rust-docs-json` is available.
    ///
    /// Returns `None` if:
    /// - Nightly toolchain is not installed
    /// - `rust-docs-json` component is not installed
    /// - Sysroot discovery fails
    pub fn discover() -> Result<Self> {
        // Get nightly sysroot
        let output = Command::new("rustc")
            .args(["+nightly", "--print", "sysroot"])
            .output()
            .context("Failed to run rustc +nightly --print sysroot")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Nightly toolchain not available: {}", stderr.trim());
        }

        let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let docs_path = sysroot.join("share/doc/rust/json");

        if !docs_path.exists() {
            anyhow::bail!(
                "rust-docs-json component not installed. Install with: rustup component add rust-docs-json --toolchain nightly"
            );
        }

        // Verify at least std.json exists
        if !docs_path.join("std.json").exists() {
            anyhow::bail!(
                "std.json not found in {}. The rust-docs-json component may be corrupted.",
                docs_path.display()
            );
        }

        // Get rustc version for display
        let version_output = Command::new("rustc")
            .args(["+nightly", "--version"])
            .output()
            .context("Failed to get rustc version")?;

        let rustc_version = String::from_utf8_lossy(&version_output.stdout)
            .trim()
            .to_string();

        tracing::info!(
            "Discovered stdlib docs at {} ({})",
            docs_path.display(),
            rustc_version
        );

        Ok(Self {
            sysroot,
            loaded: RwLock::new(HashMap::new()),
            rustc_version,
            cache_root: default_cache_root(),
        })
    }

    /// Override the cache root directory. Intended for tests.
    ///
    /// Production code should use [`Self::discover`] directly, which picks
    /// an appropriate per-user cache location via `dirs::cache_dir()`. Tests
    /// chain this method with a `TempDir` path to isolate cache state per test.
    #[must_use]
    pub fn with_cache_root(mut self, cache_root: PathBuf) -> Self {
        self.cache_root = cache_root;
        self
    }

    /// Get the path to a stdlib crate's JSON documentation.
    pub fn doc_path(&self, crate_name: &str) -> PathBuf {
        self.sysroot
            .join("share/doc/rust/json")
            .join(format!("{}.json", crate_name))
    }

    /// Get the directory where compiled search indices for stdlib crates live.
    ///
    /// The path is keyed by the `rustc` version so toolchain upgrades naturally
    /// produce a fresh cache without invalidating older entries for other builds.
    pub fn index_cache_dir(&self) -> PathBuf {
        self.cache_root
            .join("stdlib")
            .join(cache_dir_name(&self.rustc_version))
    }

    /// Get the path where a specific stdlib crate's compiled search index is cached.
    pub fn index_cache_path(&self, crate_name: &str) -> PathBuf {
        self.index_cache_dir().join(format!("{crate_name}.index"))
    }

    /// Check if a crate name is a standard library crate.
    pub fn is_stdlib_crate(crate_name: &str) -> bool {
        STDLIB_CRATES.contains(&crate_name)
    }

    /// Load a stdlib crate's documentation (lazy, cached).
    ///
    /// Returns a cached `Arc<CrateIndex>` if already loaded, otherwise
    /// loads and parses the JSON documentation.
    pub async fn load(&self, crate_name: &str) -> Result<Arc<CrateIndex>> {
        // Check if already loaded
        {
            let loaded = self.loaded.read().await;
            if let Some(index) = loaded.get(crate_name) {
                return Ok(index.clone());
            }
        }

        // Not loaded, need to load it
        let doc_path = self.doc_path(crate_name);

        if !doc_path.exists() {
            return Err(anyhow!(
                "Standard library crate '{}' documentation not found at {}",
                crate_name,
                doc_path.display()
            ));
        }

        // Load in blocking context (parsing large JSON is CPU-intensive)
        let path = doc_path.clone();
        let index = tokio::task::spawn_blocking(move || CrateIndex::load(&path))
            .await
            .context("Task join error")??;

        let index = Arc::new(index);

        // Cache it
        {
            let mut loaded = self.loaded.write().await;
            loaded.insert(
                CrateName::new_unchecked(crate_name.to_string()),
                index.clone(),
            );
        }

        tracing::debug!("Loaded stdlib docs for {}", crate_name);

        Ok(index)
    }

    /// Get list of available stdlib crates.
    pub fn available_crates(&self) -> Vec<&'static str> {
        STDLIB_CRATES
            .iter()
            .filter(|name| self.doc_path(name).exists())
            .copied()
            .collect()
    }

    /// Get the rustc version string.
    pub fn rustc_version(&self) -> &str {
        &self.rustc_version
    }

    /// Get the sysroot path.
    pub fn sysroot(&self) -> &PathBuf {
        &self.sysroot
    }

    /// Check if a specific crate's docs are available.
    pub fn has_docs(&self, crate_name: &str) -> bool {
        self.doc_path(crate_name).exists()
    }

    /// Preload commonly used crates (std) in the background.
    ///
    /// This is optional - crates are loaded lazily on first access anyway.
    pub async fn preload_common(&self) -> Result<()> {
        // Only preload std - it's the most commonly used
        // core.json is 51MB and rarely queried directly
        self.load("std").await?;
        Ok(())
    }

    /// Build a ready-to-use [`QueryContext`] for a stdlib crate.
    ///
    /// Loads the requested crate via [`Self::load`], constructs a sentinel
    /// [`WorkspaceContext`] rooted at `/`, and registers the crate index as a
    /// preloaded entry so downstream lookups short-circuit before touching
    /// the workspace. Used by the stdlib tool handlers to avoid duplicating
    /// the sentinel-workspace + preload scaffolding.
    ///
    /// # Errors
    ///
    /// Returns a user-facing error string if the crate's JSON cannot be
    /// loaded (e.g., component missing, parse failure).
    pub async fn build_query_context(&self, crate_name: &str) -> Result<QueryContext, String> {
        let crate_index = self.load(crate_name).await.map_err(|e| {
            tracing::error!(
                crate_name = %crate_name,
                error = ?e,
                "Failed to load stdlib documentation"
            );
            format!("Failed to load {crate_name} documentation: {e}")
        })?;

        let crate_name_key = CrateName::new_unchecked(crate_name);

        let mut crate_info = HashMap::new();
        crate_info.insert(
            crate_name_key.clone(),
            CrateMetadata {
                origin: CrateOrigin::Standard,
                name: crate_name_key.clone(),
                version: Some("nightly".to_string()),
                description: None,
                dev_dep: false,
                is_root_crate: false,
                used_by: vec![],
            },
        );

        // Sentinel workspace root: never touched, because `load_crate` in
        // QueryContext short-circuits on the preloaded entry below. A
        // debug_assert in `load_crate`'s cold path enforces this invariant.
        let stdlib_ctx = WorkspaceContext {
            root: PathBuf::from("/"),
            members: vec![],
            crate_info,
            root_crate: None,
        };

        let mut preloaded = HashMap::new();
        preloaded.insert(
            crate_name_key,
            PreloadedCrate {
                index: crate_index,
                source_path: self.doc_path(crate_name),
                index_cache_path: self.index_cache_path(crate_name),
            },
        );

        Ok(QueryContext::with_preloaded(
            Arc::new(stdlib_ctx),
            preloaded,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    #[test]
    fn test_is_stdlib_crate() {
        check!(StdlibDocs::is_stdlib_crate("std"));
        check!(StdlibDocs::is_stdlib_crate("core"));
        check!(StdlibDocs::is_stdlib_crate("alloc"));
        check!(!StdlibDocs::is_stdlib_crate("serde"));
        check!(!StdlibDocs::is_stdlib_crate("tokio"));
    }

    #[test]
    fn cache_dir_name_includes_version_prefix() {
        let name = cache_dir_name("rustc 1.88.0-nightly (abc1234567 2025-01-15)");
        check!(name.starts_with("1.88.0-nightly-"));
        let suffix = &name["1.88.0-nightly-".len()..];
        check!(suffix.len() == 8);
        check!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_dir_name_is_deterministic() {
        let a = cache_dir_name("rustc 1.88.0-nightly (abc1234567 2025-01-15)");
        let b = cache_dir_name("rustc 1.88.0-nightly (abc1234567 2025-01-15)");
        check!(a == b);
    }

    #[test]
    fn cache_dir_name_disambiguates_same_nominal_version() {
        let a = cache_dir_name("rustc 1.88.0-nightly (abc1234567 2025-01-15)");
        let b = cache_dir_name("rustc 1.88.0-nightly (def9876543 2025-01-16)");
        check!(a != b);
        check!(a.starts_with("1.88.0-nightly-"));
        check!(b.starts_with("1.88.0-nightly-"));
    }

    #[test]
    fn cache_dir_name_falls_back_to_pure_hash_on_bad_format() {
        let name = cache_dir_name("rustc [weird/version] (abc 2025-01-15)");
        check!(name.len() == 8);
        check!(name.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_dir_name_handles_empty_input() {
        let name = cache_dir_name("");
        check!(name.len() == 8);
        check!(name.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cache_dir_name_rejects_path_traversal() {
        // A version token containing ".." must not leak into the directory name.
        let name = cache_dir_name("rustc ..evil (abc 2025-01-15)");
        check!(!name.contains(".."));
        check!(name.len() == 8);
    }

    #[test]
    fn cache_dir_name_rejects_leading_dot_or_dash() {
        check!(!cache_dir_name("rustc .hidden (abc)").contains('.'));
        check!(cache_dir_name("rustc .hidden (abc)").len() == 8);

        let dashed = cache_dir_name("rustc -foo (abc)");
        check!(dashed.len() == 8);
    }

    #[test]
    fn cache_dir_name_caps_oversized_tokens() {
        // An absurdly long version token must not leak its length into the cache path.
        let long_version = "1.".to_string() + &"x".repeat(200);
        let input = format!("rustc {long_version} (abc)");
        let name = cache_dir_name(&input);
        check!(name.len() == 8, "oversized tokens must fall back to hash");
    }

    #[test]
    fn index_cache_path_composes_under_cache_root() {
        // Uses a non-discover StdlibDocs built by hand, since discover() requires
        // a real toolchain. We only care about the path composition logic here.
        let stdlib = StdlibDocs {
            sysroot: PathBuf::from("/unused"),
            loaded: RwLock::new(HashMap::new()),
            rustc_version: "rustc 1.88.0-nightly (abc1234567 2025-01-15)".to_string(),
            cache_root: PathBuf::from("/tmp/test-cache"),
        };

        let path = stdlib.index_cache_path("std");
        let path_str = path.to_string_lossy();

        // Layout: <cache_root>/stdlib/<version-hash>/std.index
        check!(path.starts_with("/tmp/test-cache/stdlib"));
        check!(path_str.contains("1.88.0-nightly-"));
        check!(path.file_name().unwrap() == "std.index");
    }

    #[test]
    fn different_cache_roots_produce_different_index_paths() {
        // Proves the DI wiring is honored: two StdlibDocs with different
        // cache_root values must produce distinct index paths for the same crate.
        let make = |root: &str| StdlibDocs {
            sysroot: PathBuf::from("/unused"),
            loaded: RwLock::new(HashMap::new()),
            rustc_version: "rustc 1.88.0-nightly (abc1234567 2025-01-15)".to_string(),
            cache_root: PathBuf::from(root),
        };

        let a = make("/tmp/cache-a").index_cache_path("std");
        let b = make("/tmp/cache-b").index_cache_path("std");
        check!(a != b);
        check!(a.to_string_lossy().contains("cache-a"));
        check!(b.to_string_lossy().contains("cache-b"));
    }

    #[tokio::test]
    async fn test_discover_stdlib() {
        match StdlibDocs::discover() {
            Ok(stdlib) => {
                check!(stdlib.has_docs("std"));
                check!(stdlib.has_docs("core"));
                check!(stdlib.rustc_version().contains("rustc"));
            }
            Err(e) => {
                eprintln!("Skipping test (stdlib not available): {e}");
            }
        }
    }
}
