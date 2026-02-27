//! Rustdoc JSON generation with digest-based caching.

use super::lockfile::parse_cargo_lock;
use super::metadata::validate_version;
use crate::cache::Hash;
use crate::error::Result;
use crate::search::rustdoc::CrateIndex;
use crate::types::CrateName;
use anyhow::Context;
use std::path::{Path, PathBuf};

/// Loads or regenerates rustdoc JSON for a crate using digest-based caching.
/// Regenerates documentation when source files change (workspace members) or when
/// the dependency version/checksum changes (external dependencies).
pub(crate) async fn get_docs(
    crate_name: &CrateName,
    version: Option<&str>,
    workspace_root: &Path,
    is_workspace_member: bool,
    cargo_lock_path: Option<&Path>,
) -> Result<CrateIndex> {
    use crate::cache::{
        compute_dependency_digest, compute_workspace_digest, load_digest, save_digest,
    };

    let doc_path = workspace_root
        .join("target")
        .join("doc")
        .join(format!("{}.json", crate_name.normalized()));
    let digest_path = workspace_root
        .join("target")
        .join("doc")
        .join(".digests")
        .join(format!("{}.digest.json", crate_name.normalized()));

    // Compute current digest
    let current_digest = if is_workspace_member {
        compute_workspace_digest(crate_name.as_str(), workspace_root).await?
    } else {
        // For dependencies, get checksum from Cargo.lock
        if let Some(lock_path) = cargo_lock_path {
            let crates = parse_cargo_lock(lock_path).await?;
            if let Some(pkg) = crates.get(crate_name.as_str()) {
                let checksum = pkg.checksum.unwrap_or_else(|| {
                    // Fallback for dependencies without checksums (e.g., path dependencies)
                    Hash::sha256([0u8; 32])
                });
                compute_dependency_digest(crate_name.as_str(), &pkg.version, checksum).await?
            } else {
                // Dependency not in Cargo.lock, treat as workspace member
                compute_workspace_digest(crate_name.as_str(), workspace_root).await?
            }
        } else {
            // No Cargo.lock, treat as workspace member
            compute_workspace_digest(crate_name.as_str(), workspace_root).await?
        }
    };

    // Load saved digest
    let saved_digest = load_digest(&digest_path).await;

    // Determine if regeneration is needed
    let needs_regen =
        !doc_path.exists() || saved_digest.is_none() || saved_digest.unwrap() != current_digest;

    if needs_regen {
        tracing::info!(
            crate_name = %crate_name,
            version = version,
            doc_path = %doc_path.display(),
            "Generating documentation"
        );

        generate_docs(crate_name, version, workspace_root, is_workspace_member).await?;
        save_digest(&digest_path, &current_digest).await?;

        tracing::info!(crate_name = %crate_name, "Documentation generated");
    } else {
        tracing::info!(
            crate_name = %crate_name,
            doc_path = %doc_path.display(),
            "Loading documentation from disk cache"
        );
    }

    CrateIndex::load_async(doc_path)
        .await
        .with_context(|| format!("Failed to load rustdoc JSON for '{}'", crate_name))
}

/// Invokes `cargo +nightly rustdoc` to generate JSON documentation.
/// Requires nightly toolchain. Validates inputs to prevent command injection.
///
/// For workspace members, runs `cargo rustdoc --package X` from the workspace root.
/// For external dependencies, runs `cargo rustdoc --lib` from the crate's own registry
/// source directory to avoid a nightly cargo feature resolver bug that panics when the
/// target package is only a dev-dependency of the workspace.
pub async fn generate_docs(
    crate_name: &CrateName,
    version: Option<&str>,
    workspace_root: &Path,
    is_workspace_member: bool,
) -> Result<()> {
    // Validate version to prevent command injection (crate_name already validated)
    if let Some(ver) = version {
        validate_version(ver)?;
    }

    if is_workspace_member {
        generate_docs_workspace_member(crate_name, version, workspace_root).await
    } else {
        // For external packages, find the source dir and run from there to avoid a
        // nightly cargo panic: the feature resolver fails with "did not find features
        // for (pkg, NormalOrDev)" when the target package is only a dev-dependency.
        let source_dir =
            find_registry_source_dir(crate_name, version, workspace_root).await?;
        let target_dir = workspace_root.join("target");
        generate_docs_from_source(crate_name, &source_dir, &target_dir).await
    }
}

/// Runs `cargo rustdoc --package X` for a workspace member crate.
async fn generate_docs_workspace_member(
    crate_name: &CrateName,
    version: Option<&str>,
    workspace_root: &Path,
) -> Result<()> {
    // Cargo requires the original hyphenated package name (e.g. "tracing-attributes"),
    // not the underscore-normalized form. Look it up from Cargo.lock if available.
    let canonical_name: String = {
        let lock_path = workspace_root.join("Cargo.lock");
        if lock_path.exists() {
            if let Ok(crates) = parse_cargo_lock(&lock_path).await {
                crates
                    .get(crate_name.normalized())
                    .map(|pkg| pkg.name.as_str().to_string())
                    .unwrap_or_else(|| crate_name.as_str().to_string())
            } else {
                crate_name.as_str().to_string()
            }
        } else {
            crate_name.as_str().to_string()
        }
    };

    let package_spec = if let Some(ver) = version {
        format!("{}@{}", canonical_name, ver)
    } else {
        canonical_name.clone()
    };

    let output = tokio::process::Command::new("cargo")
        .current_dir(workspace_root)
        .arg("+nightly")
        .arg("rustdoc")
        .arg("--package")
        .arg(&package_spec)
        .arg("--lib")
        .arg("--")
        .arg("-Z")
        .arg("unstable-options")
        .arg("--output-format")
        .arg("json")
        .output()
        .await
        .context("Failed to execute cargo rustdoc command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(
            package = %package_spec,
            stderr = %stderr,
            "Documentation generation failed"
        );
        tracing::error!(
            "Make sure: 1) Nightly toolchain is installed (rustup install nightly), 2) The crate exists in your dependencies"
        );
        anyhow::bail!("rustdoc command failed for crate '{}'", package_spec);
    }

    Ok(())
}

/// Runs `cargo rustdoc --lib` from the crate's own registry source directory.
///
/// This avoids the nightly cargo feature resolver panic that occurs when using
/// `--package` for a crate that is only a dev-dependency in the workspace.
async fn generate_docs_from_source(
    crate_name: &CrateName,
    source_dir: &Path,
    target_dir: &Path,
) -> Result<()> {
    let output = tokio::process::Command::new("cargo")
        .current_dir(source_dir)
        .arg("+nightly")
        .arg("rustdoc")
        .arg("--lib")
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--")
        .arg("-Z")
        .arg("unstable-options")
        .arg("--output-format")
        .arg("json")
        .output()
        .await
        .context("Failed to execute cargo rustdoc command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(
            crate_name = %crate_name,
            source_dir = %source_dir.display(),
            stderr = %stderr,
            "Documentation generation failed"
        );
        tracing::error!(
            "Make sure: 1) Nightly toolchain is installed (rustup install nightly), 2) The crate exists in your dependencies"
        );
        anyhow::bail!("rustdoc command failed for crate '{}'", crate_name.as_str());
    }

    Ok(())
}

/// Locates the unpacked source directory of a registry crate via `cargo metadata`.
///
/// Returns the parent directory of the crate's `Cargo.toml` in the cargo registry cache.
async fn find_registry_source_dir(
    crate_name: &CrateName,
    version: Option<&str>,
    workspace_root: &Path,
) -> Result<PathBuf> {
    let manifest_path = workspace_root.join("Cargo.toml");

    let output = tokio::process::Command::new("cargo")
        .current_dir(workspace_root)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output()
        .await
        .context("Failed to run cargo metadata")?;

    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse cargo metadata output")?;

    let packages = metadata["packages"]
        .as_array()
        .context("Missing 'packages' in cargo metadata output")?;

    // Normalize the name for comparison (hyphens and underscores are equivalent)
    let normalized_target = crate_name.normalized();

    for pkg in packages {
        let pkg_name = pkg["name"].as_str().unwrap_or("");
        let pkg_name_normalized = pkg_name.replace('-', "_");

        if pkg_name_normalized != normalized_target {
            continue;
        }

        // If a version was specified, require it to match
        if let Some(ver) = version {
            let pkg_version = pkg["version"].as_str().unwrap_or("");
            if pkg_version != ver {
                continue;
            }
        }

        let manifest_path_str = pkg["manifest_path"]
            .as_str()
            .context("Missing 'manifest_path' for package")?;

        let manifest = PathBuf::from(manifest_path_str);
        let source_dir = manifest
            .parent()
            .context("manifest_path has no parent directory")?
            .to_path_buf();

        return Ok(source_dir);
    }

    anyhow::bail!(
        "Package '{}' not found in cargo metadata output",
        crate_name.as_str()
    )
}
