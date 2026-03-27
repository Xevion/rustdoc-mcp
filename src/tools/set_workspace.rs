use crate::error::{ConfigError, ToolError};
use crate::types::CrateName;
use crate::workspace::{CrateMetadata, CrateOrigin, WorkspaceContext, find_workspace_root};
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
) -> Result<(PathBuf, WorkspaceContext, bool), ToolError> {
    // Validate input
    if path.trim().is_empty() {
        tracing::warn!("Empty path provided to set_workspace");
        return Err(ConfigError::NoWorkspace.into());
    }

    // Expand tilde and convert to PathBuf
    let expanded = crate::workspace::expand_tilde(&path);
    let path_buf = PathBuf::from(expanded.as_ref());

    // Canonicalize the path
    let canonical_path = tokio::fs::canonicalize(&path_buf).await.map_err(|e| {
        tracing::warn!(path = %path, error = ?e, "Failed to resolve workspace path");
        ToolError::Config(ConfigError::PathNotFound {
            path: path_buf.clone(),
        })
    })?;

    // Smart file handling - detect and handle Rust-associated files
    let metadata = tokio::fs::metadata(&canonical_path).await.map_err(|_| {
        ToolError::Config(ConfigError::PathNotFound {
            path: canonical_path.clone(),
        })
    })?;

    let resolved_dir = if metadata.is_file() {
        let filename = canonical_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        match filename.as_str() {
            "Cargo.toml" | "Cargo.lock" => canonical_path
                .parent()
                .ok_or_else(|| {
                    ToolError::Config(ConfigError::InvalidFileType {
                        path: canonical_path.clone(),
                        file_type: "file at filesystem root".to_string(),
                    })
                })?
                .to_path_buf(),
            name if name.ends_with(".rs") => {
                return Err(ConfigError::InvalidFileType {
                    path: canonical_path,
                    file_type: "Rust source file".to_string(),
                }
                .into());
            }
            _ => {
                return Err(ConfigError::InvalidFileType {
                    path: canonical_path,
                    file_type: filename,
                }
                .into());
            }
        }
    } else {
        canonical_path
    };

    // Find the workspace root
    let workspace_root = find_workspace_root(&resolved_dir).ok_or_else(|| {
        tracing::warn!(path = %resolved_dir.display(), "No Rust workspace found");
        ToolError::Config(ConfigError::NoCargoToml {
            path: resolved_dir.clone(),
        })
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

    // Locate the Cargo.toml in the workspace root
    let cargo_toml = workspace_root.join("Cargo.toml");
    if !tokio::fs::try_exists(&cargo_toml).await.unwrap_or(false) {
        return Err(ConfigError::NoCargoToml {
            path: workspace_root,
        }
        .into());
    }

    // Use cargo_metadata to discover workspace (CPU-bound, use spawn_blocking)
    let cargo_toml_clone = cargo_toml.clone();
    let metadata = tokio::task::spawn_blocking(move || {
        MetadataCommand::new()
            .manifest_path(&cargo_toml_clone)
            .exec()
            .map_err(|e| {
                ToolError::Config(ConfigError::CargoMetadata {
                    reason: e.to_string(),
                })
            })
    })
    .await
    .map_err(|e| ToolError::internal(format!("Task panicked: {e}")))??;

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

    let dep_count = metadata
        .crate_info
        .values()
        .filter(|info| info.origin == CrateOrigin::External)
        .count();

    if dep_count > 0 {
        response.push_str(&format!("Dependencies ({dep_count}):\n"));

        let mut dep_names: Vec<_> = metadata
            .crate_info
            .iter()
            .filter(|(_, info)| info.origin == CrateOrigin::External)
            .map(|(name, info)| (name.as_str(), info))
            .collect();
        dep_names.sort_by_key(|(name, _)| *name);

        for (name, info) in dep_names.iter().take(10) {
            let version = info.version.as_deref().unwrap_or("unknown");
            response.push_str(&format!("  - {name} v{version}\n"));
        }
        if dep_count > 10 {
            response.push_str(&format!("  ... and {} more\n", dep_count - 10));
        }
    }

    response
}

/// Generate comprehensive crate information with usage tracking.
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
            if dep.path.is_some() || member_names.iter().any(|m| m.matches(&dep.name)) {
                continue;
            }

            let is_dev = matches!(dep.kind, DependencyKind::Development);
            dep_usage
                .entry(dep.name.clone())
                .or_default()
                .push(CrateName::new_unchecked(package.name.to_string()));

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
fn get_rustc_version() -> Option<String> {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    version_output
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string())
}
