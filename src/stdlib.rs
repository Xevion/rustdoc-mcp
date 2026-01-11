//! Standard library documentation discovery and loading.
//!
//! Provides access to pre-generated rustdoc JSON for std, core, alloc, and other
//! standard library crates from the `rust-docs-json` nightly component.

use crate::search::CrateIndex;
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
    loaded: RwLock<HashMap<String, Arc<CrateIndex>>>,
    /// Rustc version (for display purposes)
    rustc_version: String,
}

impl std::fmt::Debug for StdlibDocs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdlibDocs")
            .field("sysroot", &self.sysroot)
            .field("rustc_version", &self.rustc_version)
            .field("loaded_count", &self.loaded.blocking_read().len())
            .finish()
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
        })
    }

    /// Get the path to a stdlib crate's JSON documentation.
    pub fn doc_path(&self, crate_name: &str) -> PathBuf {
        self.sysroot
            .join("share/doc/rust/json")
            .join(format!("{}.json", crate_name))
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
            loaded.insert(crate_name.to_string(), index.clone());
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_stdlib_crate() {
        assert!(StdlibDocs::is_stdlib_crate("std"));
        assert!(StdlibDocs::is_stdlib_crate("core"));
        assert!(StdlibDocs::is_stdlib_crate("alloc"));
        assert!(!StdlibDocs::is_stdlib_crate("serde"));
        assert!(!StdlibDocs::is_stdlib_crate("tokio"));
    }

    #[tokio::test]
    async fn test_discover_stdlib() {
        // This test requires nightly + rust-docs-json to be installed
        match StdlibDocs::discover() {
            Ok(stdlib) => {
                assert!(stdlib.has_docs("std"));
                assert!(stdlib.has_docs("core"));
                assert!(stdlib.rustc_version().contains("rustc"));
            }
            Err(e) => {
                // Skip test if not available
                eprintln!("Skipping test (stdlib not available): {}", e);
            }
        }
    }
}
