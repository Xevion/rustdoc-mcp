//! Shared test fixtures and utilities for integration tests.
//!
//! # Test Isolation Strategy
//!
//! Tests use isolated workspaces to prevent cache interference. Each test gets:
//! - A fresh temporary directory with copied rustdoc JSON files
//! - Its own `DocState` with empty in-memory LRU cache
//! - No pre-existing `.index` files (cold cache state)
//!
//! # Available Fixtures
//!
//! - `isolated_workspace`: Creates a fully isolated test environment (recommended)
//! - `isolated_workspace_with_serde`: Includes serde and serde_json dependencies
//! - `shared_context`: Uses the real `target/doc/` directory (for specific warm-cache tests)
//!
//! # Testing Warm Cache Behavior
//!
//! To test with a warm cache, use the `warm_cache()` helper:
//! ```ignore
//! async fn my_test(isolated_workspace: IsolatedWorkspace) {
//!     warm_cache(&isolated_workspace.context, &["rustdoc-mcp"]).await;
//!     // Now cache is warm, test warm-cache behavior
//! }
//! ```
//!
//! # Shared Infrastructure
//!
//! [`TempWorkspace`] provides a reusable temp directory abstraction for any test
//! that needs filesystem isolation (not just MCP server tests).

use rstest::fixture;
use rustdoc_mcp::tools::search::{SearchRequest, handle_search};
use rustdoc_mcp::{CrateMetadata, CrateOrigin, DocState, WorkspaceContext};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

/// Returns the project root directory (where Cargo.toml lives).
pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A temporary workspace directory for test isolation.
///
/// Provides basic filesystem operations within a temp directory that is
/// automatically cleaned up when dropped. Use this as a building block
/// for test fixtures that need filesystem isolation.
///
/// # Example
///
/// ```ignore
/// let workspace = TempWorkspace::new();
/// workspace.create_file("src/main.rs", "fn main() {}");
/// workspace.create_dir("target/doc");
/// assert!(workspace.path().join("src/main.rs").exists());
/// ```
#[allow(dead_code)] // Methods used across different integration test crates
pub struct TempWorkspace {
    _temp: TempDir,
    root: PathBuf,
}

#[allow(dead_code)] // Methods used across different integration test crates
impl TempWorkspace {
    /// Creates a new empty temporary workspace.
    pub fn new() -> Self {
        let temp = TempDir::new().expect("Failed to create temp directory");
        let root = temp.path().to_path_buf();
        Self { _temp: temp, root }
    }

    /// Returns the root path of this workspace.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Creates a directory (and all parent directories) within this workspace.
    ///
    /// # Panics
    /// Panics if directory creation fails.
    pub fn create_dir(&self, path: &str) {
        let full_path = self.root.join(path);
        std::fs::create_dir_all(&full_path)
            .unwrap_or_else(|e| panic!("Failed to create directory '{}': {}", path, e));
    }

    /// Creates a file with the given content within this workspace.
    ///
    /// Parent directories are created automatically if they don't exist.
    ///
    /// # Panics
    /// Panics if file creation fails.
    pub fn create_file(&self, path: &str, content: &str) {
        let full_path = self.root.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                panic!("Failed to create parent directory for '{}': {}", path, e)
            });
        }
        std::fs::write(&full_path, content)
            .unwrap_or_else(|e| panic!("Failed to write file '{}': {}", path, e));
    }

    /// Copies a file from the real filesystem into this workspace.
    ///
    /// # Panics
    /// Panics if copying fails.
    pub fn copy_file(&self, source: &Path, dest_relative: &str) {
        let dest = self.root.join(dest_relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                panic!(
                    "Failed to create parent directory for '{}': {}",
                    dest_relative, e
                )
            });
        }
        std::fs::copy(source, &dest).unwrap_or_else(|e| {
            panic!(
                "Failed to copy '{}' to '{}': {}",
                source.display(),
                dest_relative,
                e
            )
        });
    }

    /// Creates a Cargo.toml file with either workspace or package configuration.
    pub fn create_cargo_toml(&self, path: &str, is_workspace: bool) {
        let content = if is_workspace {
            r#"
[workspace]
members = ["member1", "member2"]

[workspace.package]
version = "0.1.0"
edition = "2024"
"#
        } else {
            r#"
[package]
name = "test-package"
version = "0.1.0"
edition = "2024"
"#
        };
        self.create_file(path, content);
    }

    /// Creates a minimal .git directory structure for testing git detection.
    pub fn create_git_repo(&self, path: &str) {
        let git_path = self.root.join(path).join(".git");
        std::fs::create_dir_all(&git_path).expect("Failed to create .git directory");
        std::fs::create_dir_all(git_path.join("refs")).expect("Failed to create refs");
        std::fs::create_dir_all(git_path.join("objects")).expect("Failed to create objects");
        std::fs::write(git_path.join("HEAD"), "ref: refs/heads/main")
            .expect("Failed to write HEAD");
    }

    /// Creates a git submodule marker (a .git file pointing to parent).
    pub fn create_git_submodule(&self, path: &str) {
        let git_file = self.root.join(path).join(".git");
        if let Some(parent) = git_file.parent() {
            std::fs::create_dir_all(parent).expect("Failed to create submodule directory");
        }
        std::fs::write(&git_file, "gitdir: ../.git/modules/submodule")
            .expect("Failed to write .git file");
    }
}

impl Default for TempWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// An isolated workspace for MCP server testing.
///
/// Copies rustdoc JSON to a temp directory to ensure fresh index builds.
/// Each test using this gets complete isolation from other tests.
///
/// Composes [`TempWorkspace`] with MCP-specific setup (DocState).
#[allow(dead_code)] // Fields used across different integration test crates
pub struct IsolatedWorkspace {
    workspace: TempWorkspace,
    pub state: Arc<DocState>,
}

#[allow(dead_code)] // Methods used across different integration test crates
impl IsolatedWorkspace {
    /// Creates a new isolated workspace with default crates (rustdoc-mcp only).
    ///
    /// For tests that need external dependencies, use `with_deps()` instead.
    pub fn new() -> Self {
        Self::with_deps(&["rustdoc-mcp"])
    }

    /// Creates an isolated workspace with specified crates.
    ///
    /// Copies rustdoc JSON for the specified crates and registers them
    /// in the workspace context.
    ///
    /// # Arguments
    /// * `crates` - List of crate names to include (e.g., `["rustdoc-mcp", "serde"]`)
    pub fn with_deps(crates: &[&str]) -> Self {
        let workspace = TempWorkspace::new();
        let project = project_root();

        // Create target/doc structure
        workspace.create_dir("target/doc");

        // Copy rustdoc JSON files for requested crates
        let source_doc_dir = project.join("target/doc");
        if source_doc_dir.exists() {
            for crate_name in crates {
                // Normalize crate name for file lookup (hyphens → underscores)
                let normalized = crate_name.replace('-', "_");
                let json_file = format!("{}.json", normalized);
                let source_path = source_doc_dir.join(&json_file);

                if source_path.exists() {
                    let dest_relative = format!("target/doc/{}", json_file);
                    workspace.copy_file(&source_path, &dest_relative);
                }
            }
        }

        // Note: .digests/ directory is NOT copied because:
        // - JSON files are copied, so doc_path.exists() returns true
        // - Digest validation only runs when docs need regeneration
        // - Since JSON exists, get_docs() is never called in tests

        // Copy Cargo.toml from real project (for WorkspaceContext)
        let source_cargo = project.join("Cargo.toml");
        workspace.copy_file(&source_cargo, "Cargo.toml");

        // Copy Cargo.lock from real project
        let source_lock = project.join("Cargo.lock");
        if source_lock.exists() {
            workspace.copy_file(&source_lock, "Cargo.lock");
        }

        // Build crate_info for all requested crates
        let mut crate_info = HashMap::new();

        for crate_name in crates {
            let is_local = *crate_name == "rustdoc-mcp";
            crate_info.insert(
                crate_name.to_string(),
                CrateMetadata {
                    origin: if is_local {
                        CrateOrigin::Local
                    } else {
                        CrateOrigin::External
                    },
                    version: Some(if is_local {
                        "0.2.0".to_string()
                    } else {
                        "1.0".to_string()
                    }),
                    description: None,
                    dev_dep: false,
                    name: crate_name.to_string(),
                    is_root_crate: is_local,
                    used_by: vec![],
                },
            );
        }

        let root = workspace.path().to_path_buf();
        let metadata = WorkspaceContext {
            root: root.clone(),
            members: vec!["rustdoc-mcp".to_string()],
            crate_info,
            root_crate: Some("rustdoc-mcp".to_string()),
        };

        let state = Arc::new(DocState::new(None));

        // Set up cargo lock path if it exists
        let cargo_lock = root.join("Cargo.lock");
        let cargo_lock = if cargo_lock.exists() {
            Some(cargo_lock)
        } else {
            None
        };

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                state
                    .set_workspace(root.clone(), metadata, cargo_lock)
                    .await;
            });
        });

        Self { workspace, state }
    }

    /// Returns the root path of this workspace.
    pub fn root(&self) -> &Path {
        self.workspace.path()
    }
}

impl Default for IsolatedWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates an isolated workspace for testing.
///
/// This is the **recommended fixture** for most tests. It provides:
/// - Complete isolation from other tests (temp directory)
/// - No pre-existing cache files (cold state)
/// - Support for rustdoc-mcp crate only (use `isolated_workspace_with_serde` for external deps)
///
/// Tests using this fixture can run in parallel without interference.
///
/// **Important**: This returns `IsolatedWorkspace` not just `Arc<DocState>` because
/// the temp directory must stay alive for the duration of the test. Use `.state`
/// to get the DocState for tool handlers.
#[fixture]
pub fn isolated_workspace() -> IsolatedWorkspace {
    IsolatedWorkspace::new()
}

/// Creates an isolated workspace with serde dependencies for testing.
///
/// Same as `isolated_workspace` but also includes serde and serde_json crates.
/// Also includes serde_core for cross-crate re-export resolution.
#[fixture]
pub fn isolated_workspace_with_serde() -> IsolatedWorkspace {
    IsolatedWorkspace::with_deps(&["rustdoc-mcp", "serde", "serde_json", "serde_core"])
}

/// Creates an `Arc<DocState>` using the real project directory (shared state).
///
/// **Use with caution!** This fixture uses the actual `target/doc/` directory,
/// which means:
/// - Cache files may exist from previous test runs
/// - Tests may interfere with each other
/// - Tests using this should be marked with `#[serial]`
///
/// Prefer `isolated_workspace` unless you specifically need to test shared-state behavior.
#[fixture]
pub fn shared_state() -> Arc<DocState> {
    let project_root = project_root();

    // Build crate_info for all crates we want to test against
    let mut crate_info = HashMap::new();

    // The local crate
    crate_info.insert(
        "rustdoc-mcp".to_string(),
        CrateMetadata {
            origin: CrateOrigin::Local,
            version: Some("0.2.0".to_string()),
            description: Some("MCP server for Rust documentation".to_string()),
            dev_dep: false,
            name: "rustdoc-mcp".to_string(),
            is_root_crate: true,
            used_by: vec![],
        },
    );

    // External dependencies we test against
    crate_info.insert(
        "serde".to_string(),
        CrateMetadata {
            origin: CrateOrigin::External,
            version: Some("1.0".to_string()),
            description: None,
            dev_dep: false,
            name: "serde".to_string(),
            is_root_crate: false,
            used_by: vec![],
        },
    );

    crate_info.insert(
        "serde_json".to_string(),
        CrateMetadata {
            origin: CrateOrigin::External,
            version: Some("1.0".to_string()),
            description: None,
            dev_dep: false,
            name: "serde_json".to_string(),
            is_root_crate: false,
            used_by: vec![],
        },
    );

    let metadata = WorkspaceContext {
        root: project_root.clone(),
        members: vec!["rustdoc-mcp".to_string()],
        crate_info,
        root_crate: Some("rustdoc-mcp".to_string()),
    };

    let state = Arc::new(DocState::new(None));

    let cargo_lock = project_root.join("Cargo.lock");
    let cargo_lock = if cargo_lock.exists() {
        Some(cargo_lock)
    } else {
        None
    };

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .set_workspace(project_root, metadata, cargo_lock)
                .await;
        });
    });

    state
}

/// Warms the search index cache by triggering index builds.
///
/// This is useful for tests that need to verify warm-cache behavior.
/// After calling this, the specified crates will have their `.index`
/// files created in the workspace's `target/doc/` directory.
///
/// # Arguments
/// * `state` - The DocState to warm
/// * `crates` - List of crate names to warm (e.g., `["rustdoc-mcp"]`)
///
/// # Example
/// ```ignore
/// #[rstest]
/// #[tokio::test(flavor = "multi_thread")]
/// async fn test_warm_cache_behavior(isolated_workspace: IsolatedWorkspace) {
///     // Warm the cache first
///     warm_cache(&isolated_workspace.state, &["rustdoc-mcp"]).await;
///
///     // Now test with warm cache
///     let result = handle_search(&isolated_workspace.state, ...).await;
/// }
/// ```
#[allow(dead_code)] // Used in search_test.rs
pub async fn warm_cache(state: &Arc<DocState>, crates: &[&str]) {
    for crate_name in crates {
        // Trigger a search to force index build
        // Using a minimal query that will still trigger indexing
        let _ = handle_search(
            state,
            SearchRequest {
                query: "_warmup_".to_string(),
                crate_name: crate_name.to_string(),
                limit: Some(1),
            },
        )
        .await;
    }
}
