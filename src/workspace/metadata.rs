//! Cargo metadata execution and dependency resolution.

use crate::error::Result;
use anyhow::Context;
use cargo_metadata::{DependencyKind, MetadataCommand};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Validate crate name contains only safe characters
pub fn validate_crate_name(name: &str) -> Result<()> {
    let crate_name_regex = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();
    if !crate_name_regex.is_match(name) {
        anyhow::bail!(
            "Invalid crate name '{}': must contain only alphanumeric characters, hyphens, and underscores",
            name
        );
    }
    Ok(())
}

/// Validate version string matches semver format
pub fn validate_version(version: &str) -> Result<()> {
    let version_regex = regex::Regex::new(r"^\d+(\.\d+){0,2}").unwrap();
    if !version_regex.is_match(version) {
        anyhow::bail!(
            "Invalid version '{}': must be in semver format (e.g., 1.0.0)",
            version
        );
    }
    Ok(())
}

/// Extracts resolved dependency versions from cargo metadata.
/// Returns only normal (non-dev, non-build) dependencies of workspace members.
pub async fn get_resolved_versions(workspace_root: &Path) -> Result<HashMap<String, String>> {
    let workspace_root = workspace_root.to_path_buf();
    let metadata = tokio::task::spawn_blocking(move || {
        MetadataCommand::new()
            .current_dir(&workspace_root)
            .exec()
            .context("Failed to run cargo metadata")
    })
    .await
    .context("Task panicked")??;

    let mut direct_deps: HashMap<String, String> = HashMap::new();

    // Get all workspace crate IDs
    let workspace_pkg_ids: HashSet<_> = metadata.workspace_members.iter().collect();

    // For each workspace crate, collect its direct dependencies
    for pkg in &metadata.packages {
        if workspace_pkg_ids.contains(&pkg.id) {
            for dep in &pkg.dependencies {
                if dep.kind == DependencyKind::Normal {
                    // Find the resolved version from crates
                    if let Some(dep_pkg) = metadata.packages.iter().find(|p| p.name == dep.name) {
                        direct_deps
                            .entry(dep_pkg.name.to_string())
                            .or_insert(dep_pkg.version.to_string());
                    }
                }
            }
        }
    }

    Ok(direct_deps)
}

/// Extracts all dependency names from Cargo.toml (dependencies, dev-dependencies, build-dependencies).
pub fn extract_dependencies(cargo_toml_path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(cargo_toml_path)
        .with_context(|| format!("Failed to read Cargo.toml at {}", cargo_toml_path.display()))?;
    let toml_value: toml::Value = toml::from_str(&content).context("Failed to parse Cargo.toml")?;

    let mut crates = HashSet::new();

    let mut extract_from_table = |table: &toml::Value| {
        if let Some(deps) = table.as_table() {
            for (name, _value) in deps {
                crates.insert(name.clone());
            }
        }
    };

    if let Some(deps) = toml_value.get("dependencies") {
        extract_from_table(deps);
    }

    if let Some(deps) = toml_value.get("dev-dependencies") {
        extract_from_table(deps);
    }

    if let Some(deps) = toml_value.get("build-dependencies") {
        extract_from_table(deps);
    }

    let mut result: Vec<String> = crates.into_iter().collect();
    result.sort();
    Ok(result)
}
