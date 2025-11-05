//! Rustdoc JSON generation with digest-based caching.

use super::lockfile::parse_cargo_lock;
use super::metadata::{validate_crate_name, validate_version};
use crate::cache::Hash;
use crate::error::Result;
use crate::search::rustdoc::CrateIndex;
use anyhow::Context;
use std::path::Path;
use tracing::{debug, error, info};

/// Loads or regenerates rustdoc JSON for a crate using digest-based caching.
/// Regenerates documentation when source files change (workspace members) or when
/// the dependency version/checksum changes (external dependencies).
pub async fn get_docs(
    crate_name: &str,
    version: Option<&str>,
    workspace_root: &Path,
    is_workspace_member: bool,
    cargo_lock_path: Option<&Path>,
) -> Result<CrateIndex> {
    use crate::cache::{
        compute_dependency_digest, compute_workspace_digest, load_digest, save_digest,
    };

    let normalized_name = crate_name.replace('-', "_");
    let doc_path = workspace_root
        .join("target")
        .join("doc")
        .join(format!("{}.json", normalized_name));
    let digest_path = workspace_root
        .join("target")
        .join("doc")
        .join(".digests")
        .join(format!("{}.digest.json", normalized_name));

    // Compute current digest
    let current_digest = if is_workspace_member {
        compute_workspace_digest(crate_name, workspace_root).await?
    } else {
        // For dependencies, get checksum from Cargo.lock
        if let Some(lock_path) = cargo_lock_path {
            let crates = parse_cargo_lock(lock_path).await?;
            if let Some(pkg) = crates.get(crate_name) {
                let checksum = pkg.checksum.unwrap_or_else(|| {
                    // Fallback for dependencies without checksums (e.g., path dependencies)
                    Hash::sha256([0u8; 32])
                });
                compute_dependency_digest(crate_name, &pkg.version, checksum).await?
            } else {
                // Dependency not in Cargo.lock, treat as workspace member
                compute_workspace_digest(crate_name, workspace_root).await?
            }
        } else {
            // No Cargo.lock, treat as workspace member
            compute_workspace_digest(crate_name, workspace_root).await?
        }
    };

    // Load saved digest
    let saved_digest = load_digest(&digest_path).await;

    // Determine if regeneration is needed
    let needs_regen =
        !doc_path.exists() || saved_digest.is_none() || saved_digest.unwrap() != current_digest;

    if needs_regen {
        debug!("Documentation needs regeneration for {}", crate_name);
        info!(
            "Generating documentation for {}{}",
            crate_name,
            version.map(|v| format!("@{}", v)).unwrap_or_default()
        );

        generate_docs(crate_name, version, workspace_root).await?;
        save_digest(&digest_path, &current_digest).await?;

        info!("Documentation generated");
    } else {
        debug!("Using cached documentation for {}", crate_name);
    }

    CrateIndex::load(&doc_path)
}

/// Invokes `cargo +nightly rustdoc` to generate JSON documentation.
/// Requires nightly toolchain. Validates inputs to prevent command injection.
pub async fn generate_docs(
    crate_name: &str,
    version: Option<&str>,
    workspace_root: &Path,
) -> Result<()> {
    // Validate inputs to prevent command injection
    validate_crate_name(crate_name)?;
    if let Some(ver) = version {
        validate_version(ver)?;
    }

    let package_spec = if let Some(ver) = version {
        format!("{}@{}", crate_name, ver)
    } else {
        crate_name.to_string()
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
        error!(
            "Failed to generate documentation for '{}': {}",
            package_spec, stderr
        );
        error!(
            "Make sure: 1) Nightly toolchain is installed (rustup install nightly), 2) The crate exists in your dependencies"
        );
        anyhow::bail!("rustdoc command failed for crate '{}'", package_spec);
    }

    Ok(())
}
