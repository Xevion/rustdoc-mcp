use crate::cargo::get_resolved_versions;
use crate::context::WorkspaceMetadata;
use anyhow::{anyhow, Result};
use cargo_metadata::MetadataCommand;
use std::path::PathBuf;

/// Configure the workspace by discovering Rust project metadata.
///
/// Locates Cargo.toml, runs cargo metadata to discover workspace members,
/// and resolves all dependencies with their exact versions.
pub fn execute_set_workspace(path: String) -> Result<(PathBuf, WorkspaceMetadata)> {
    // Expand tilde and convert to PathBuf
    let expanded = shellexpand::tilde(&path);
    let path_buf = PathBuf::from(expanded.as_ref());

    // Canonicalize the path
    let canonical_path = std::fs::canonicalize(&path_buf)
        .map_err(|e| anyhow!("Failed to resolve path '{}': {}", path, e))?;

    // Verify it's a directory
    if !canonical_path.is_dir() {
        return Err(anyhow!("Path is not a directory: {}", canonical_path.display()));
    }

    // Look for Cargo.toml
    let cargo_toml = canonical_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(anyhow!(
            "No Cargo.toml found in directory: {}",
            canonical_path.display()
        ));
    }

    // Use cargo_metadata to discover workspace
    let metadata = MetadataCommand::new()
        .manifest_path(&cargo_toml)
        .exec()
        .map_err(|e| anyhow!("Failed to get cargo metadata: {}", e))?;

    // Extract workspace members using typed API
    let members: Vec<String> = metadata
        .workspace_packages()
        .iter()
        .map(|pkg| pkg.name.to_string())
        .collect();

    // Get dependencies with versions
    let dependencies = get_resolved_versions(&canonical_path)
        .map(|deps| deps.into_iter().collect())
        .unwrap_or_default();

    let workspace_metadata = WorkspaceMetadata {
        root: canonical_path.clone(),
        members,
        dependencies,
    };

    Ok((canonical_path, workspace_metadata))
}

/// Format a user-friendly response showing workspace configuration results.
pub fn format_response(path: &PathBuf, metadata: &WorkspaceMetadata) -> String {
    let mut response = format!("Workspace configured: {}\n\n", path.display());

    if !metadata.members.is_empty() {
        response.push_str(&format!("Workspace members ({}):\n", metadata.members.len()));
        for member in &metadata.members {
            response.push_str(&format!("  - {}\n", member));
        }
        response.push('\n');
    }

    if !metadata.dependencies.is_empty() {
        response.push_str(&format!("Dependencies ({}):\n", metadata.dependencies.len()));
        let mut sorted_deps = metadata.dependencies.clone();
        sorted_deps.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, version) in sorted_deps.iter().take(10) {
            response.push_str(&format!("  - {} v{}\n", name, version));
        }
        if metadata.dependencies.len() > 10 {
            response.push_str(&format!("  ... and {} more\n", metadata.dependencies.len() - 10));
        }
    }

    response
}
