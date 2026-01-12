use crate::error::Result;
use crate::types::CrateName;
use crate::workspace::{CrateMetadata, CrateOrigin, WorkspaceContext, find_workspace_root};
use anyhow::anyhow;
use cargo_metadata::{DependencyKind, Metadata, MetadataCommand};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Configure the workspace by discovering Rust project metadata.
///
/// Locates Cargo.toml, runs cargo metadata to discover workspace members,
/// and resolves all dependencies with their exact versions.
///
/// Returns a tuple of (canonical_path, workspace_context, changed) where
/// `changed` indicates whether the workspace was actually changed.
#[tracing::instrument(skip_all, fields(path = %path))]
pub(crate) async fn handle_set_workspace(
    path: String,
    current_workspace: Option<&Path>,
) -> Result<(PathBuf, WorkspaceContext, bool)> {
    // Validate input - check for empty or whitespace-only paths
    if path.trim().is_empty() {
        tracing::warn!("Empty path provided to set_workspace");
        return Err(anyhow!(
            "Path cannot be empty. Please provide a path to your Rust project directory."
        ));
    }

    // Expand tilde and convert to PathBuf
    let expanded = crate::workspace::expand_tilde(&path);
    let path_buf = PathBuf::from(expanded.as_ref());

    // Canonicalize the path
    let canonical_path = tokio::fs::canonicalize(&path_buf).await.map_err(|e| {
        tracing::warn!(path = %path, error = %e, "Failed to resolve workspace path");
        anyhow!(
            "Failed to resolve path '{}': {}. Please check the path exists and is accessible.",
            path,
            e
        )
    })?;

    // Smart file handling - detect and handle Rust-associated files
    let metadata = tokio::fs::metadata(&canonical_path).await.map_err(|e| {
        anyhow!(
            "Failed to access path '{}': {}",
            canonical_path.display(),
            e
        )
    })?;

    let resolved_dir = if metadata.is_file() {
        let filename = canonical_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        match filename {
            "Cargo.toml" | "Cargo.lock" => {
                // Valid Rust project files - use parent directory
                canonical_path
                    .parent()
                    .ok_or_else(|| {
                        anyhow!(
                            "Cannot use file at filesystem root: {}",
                            canonical_path.display()
                        )
                    })?
                    .to_path_buf()
            }
            name if name.ends_with(".rs") => {
                return Err(anyhow!(
                    "Source files cannot be used as workspace paths. \
                     Please provide the project directory (containing Cargo.toml)."
                ));
            }
            _ => {
                return Err(anyhow!(
                    "File `{}` is not a Rust project file. \
                     Please provide a directory path or a Cargo.toml/Cargo.lock file.",
                    filename
                ));
            }
        }
    } else {
        canonical_path
    };

    // Find the workspace root (handles member crates automatically)
    // This walks upward to find a Cargo.toml with [workspace] section
    let workspace_root = find_workspace_root(&resolved_dir).ok_or_else(|| {
        tracing::warn!(path = %resolved_dir.display(), "No Rust workspace found");
        anyhow!(
            "No valid Rust workspace found at or above: `{}`. \
             Please ensure the directory contains a Cargo.toml file.",
            resolved_dir.display()
        )
    })?;

    tracing::debug!(
        requested = %resolved_dir.display(),
        resolved = %workspace_root.display(),
        "Found workspace root"
    );

    // Check if workspace has changed
    let workspace_changed = current_workspace
        .map(|current| current != workspace_root.as_path())
        .unwrap_or(true);

    // If workspace hasn't changed, we could return early with cached data
    // For now, we still regenerate to ensure fresh metadata
    // TODO: Add caching to avoid regenerating docs when workspace unchanged

    // Locate the Cargo.toml in the workspace root
    let cargo_toml = workspace_root.join("Cargo.toml");
    if !tokio::fs::try_exists(&cargo_toml).await.unwrap_or(false) {
        return Err(anyhow!(
            "Internal error: find_workspace_root returned a path without Cargo.toml: `{}`",
            workspace_root.display()
        ));
    }

    // Use cargo_metadata to discover workspace (CPU-bound, use spawn_blocking)
    let cargo_toml_clone = cargo_toml.clone();
    let metadata = tokio::task::spawn_blocking(move || {
        MetadataCommand::new()
            .manifest_path(&cargo_toml_clone)
            .exec()
            .map_err(|e| anyhow!("Failed to get cargo metadata: `{}`", e))
    })
    .await
    .map_err(|e| anyhow!("Task panicked: `{}`", e))??;

    // Extract workspace members using typed API
    let members: Vec<CrateName> = metadata
        .workspace_packages()
        .iter()
        .map(|pkg| CrateName::new_unchecked(pkg.name.to_string()))
        .collect();

    let workspace_ctx = WorkspaceContext {
        root: workspace_root.clone(),
        members: members.clone(),
        crate_info: collect_crate_metadata(&metadata, &members),
        root_crate: metadata
            .root_package()
            .map(|p| CrateName::new_unchecked(p.name.to_string())),
    };

    Ok((workspace_root, workspace_ctx, workspace_changed))
}

/// Format a user-friendly response showing workspace configuration results.
///
/// Includes contextual messaging based on whether the workspace changed and what the
/// previous workspace was.
pub(crate) fn format_response(
    path: &Path,
    metadata: &WorkspaceContext,
    old_workspace: Option<&Path>,
    changed: bool,
) -> String {
    let header = if !changed {
        format!("Workspace already set to: `{}`\n\n", path.display())
    } else if let Some(old) = old_workspace {
        format!(
            "Workspace changed:\n  From: `{}`\n  To:   `{}`\n\n",
            old.display(),
            path.display()
        )
    } else {
        format!("Workspace set to: `{}`\n\n", path.display())
    };

    let mut response = header;

    if !metadata.members.is_empty() {
        response.push_str(&format!(
            "Workspace members ({}):\n",
            metadata.members.len()
        ));
        for member in &metadata.members {
            response.push_str(&format!("  - {}\n", member));
        }
        response.push('\n');
    }

    // Count non-workspace dependencies
    let dep_count = metadata
        .crate_info
        .values()
        .filter(|info| info.origin == CrateOrigin::External)
        .count();

    if dep_count > 0 {
        response.push_str(&format!("Dependencies ({}):\n", dep_count));

        // Collect and sort dependency names
        let mut dep_names: Vec<_> = metadata
            .crate_info
            .iter()
            .filter(|(_, info)| info.origin == CrateOrigin::External)
            .map(|(name, info)| (name.as_str(), info))
            .collect();
        dep_names.sort_by_key(|(name, _)| *name);

        for (name, info) in dep_names.iter().take(10) {
            let version = info.version.as_deref().unwrap_or("unknown");
            response.push_str(&format!("  - {} v{}\n", name, version));
        }
        if dep_count > 10 {
            response.push_str(&format!("  ... and {} more\n", dep_count - 10));
        }
    }

    response
}

/// Generate comprehensive crate information with usage tracking.
///
/// Collects workspace members, dependencies, and standard library crates,
/// tracking which workspace members use each dependency.
fn collect_crate_metadata(
    metadata: &Metadata,
    member_names: &[CrateName],
) -> HashMap<CrateName, CrateMetadata> {
    let mut crates = HashMap::new();
    let is_workspace = member_names.len() > 1;
    let root_package_name = metadata.root_package().map(|p| p.name.as_str());

    // 1. Add workspace members
    for package in metadata.workspace_packages() {
        crates.insert(
            CrateName::new_unchecked(package.name.to_string()),
            CrateMetadata {
                origin: CrateOrigin::Local,
                name: CrateName::new_unchecked(package.name.to_string()),
                version: Some(package.version.to_string()),
                description: package.description.clone(),
                dev_dep: false,
                is_root_crate: !is_workspace
                    && root_package_name.is_some_and(|rp| package.name == rp),
                used_by: vec![],
            },
        );
    }

    // 2. Track dependency usage
    let mut dep_usage: BTreeMap<String, Vec<CrateName>> = BTreeMap::new();
    let mut dep_dev_status: BTreeMap<String, bool> = BTreeMap::new();

    for package in metadata.workspace_packages() {
        for dep in &package.dependencies {
            // Skip workspace-internal dependencies (path dependencies or members)
            if dep.path.is_some() || member_names.iter().any(|m| m.matches(&dep.name)) {
                continue;
            }

            let is_dev = matches!(dep.kind, DependencyKind::Development);
            dep_usage
                .entry(dep.name.clone())
                .or_default()
                .push(CrateName::new_unchecked(package.name.to_string()));

            // Track if ANY workspace member uses this as a dev dep
            let current_dev = dep_dev_status.get(&dep.name).copied().unwrap_or(false);
            dep_dev_status.insert(dep.name.clone(), current_dev || is_dev);
        }
    }

    // 3. Convert dependencies to CrateMetadata
    for (dep_name, using_crates) in dep_usage {
        let dev_dep = dep_dev_status.get(&dep_name).copied().unwrap_or(false);
        let pkg_metadata = metadata.packages.iter().find(|p| p.name == dep_name);

        crates.insert(
            CrateName::new_unchecked(dep_name.clone()),
            CrateMetadata {
                origin: CrateOrigin::External,
                name: CrateName::new_unchecked(dep_name),
                version: pkg_metadata.map(|p| p.version.to_string()),
                description: pkg_metadata.and_then(|p| p.description.clone()),
                dev_dep,
                is_root_crate: false,
                used_by: using_crates,
            },
        );
    }

    // 4. Add standard library crates
    if let Some(rustc_version) = get_rustc_version() {
        for stdlib_name in ["std", "core", "alloc", "proc_macro", "test"] {
            crates.insert(
                CrateName::new_unchecked(stdlib_name),
                CrateMetadata {
                    origin: CrateOrigin::Standard,
                    name: CrateName::new_unchecked(stdlib_name),
                    version: Some(rustc_version.clone()),
                    description: None,
                    dev_dep: false,
                    is_root_crate: false,
                    used_by: vec![],
                },
            );
        }
    }

    crates
}

/// Detect the rustc version for standard library crates.
///
/// Returns `None` if rustc is not available or version cannot be determined.
fn get_rustc_version() -> Option<String> {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    // Parse "rustc 1.75.0 (abcdef123 2024-01-01)" -> "1.75.0"
    version_output
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string())
}
