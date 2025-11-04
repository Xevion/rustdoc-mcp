use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;
use walkdir::WalkDir;

/// Fingerprint for tracking when documentation needs regeneration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocFingerprint {
    /// Hash of rustc version output (invalidates all docs on toolchain change)
    pub rustc_version_hash: u64,
    /// Type-specific fingerprint data
    pub crate_type: CrateType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrateType {
    WorkspaceMember {
        /// Hash of Cargo.toml contents
        manifest_hash: u64,
        /// Combined hash of all source files
        source_hash: u64,
        /// Sorted list of enabled features
        features: Vec<String>,
    },
    Dependency {
        /// Crate version
        version: String,
        /// SHA256 checksum from Cargo.lock (guarantees immutability)
        checksum: String,
    },
}

/// Compute fingerprint for a workspace member crate
pub fn compute_workspace_fingerprint(
    _crate_name: &str,
    workspace_root: &Path,
) -> Result<DocFingerprint, Box<dyn std::error::Error>> {
    let rustc_version_hash = get_rustc_version_hash()?;

    // Hash Cargo.toml
    let manifest_path = workspace_root.join("Cargo.toml");
    let manifest_hash = hash_file(&manifest_path)?;

    // Hash all source files
    let src_dir = workspace_root.join("src");
    let source_hash = hash_directory(&src_dir)?;

    // For now, we don't track features (would need to be passed in)
    // This is acceptable because feature changes usually require explicit cargo invocations
    let features = Vec::new();

    Ok(DocFingerprint {
        rustc_version_hash,
        crate_type: CrateType::WorkspaceMember {
            manifest_hash,
            source_hash,
            features,
        },
    })
}

/// Compute fingerprint for a dependency crate
pub fn compute_dependency_fingerprint(
    _crate_name: &str,
    version: &str,
    checksum: &str,
) -> Result<DocFingerprint, Box<dyn std::error::Error>> {
    let rustc_version_hash = get_rustc_version_hash()?;

    Ok(DocFingerprint {
        rustc_version_hash,
        crate_type: CrateType::Dependency {
            version: version.to_string(),
            checksum: checksum.to_string(),
        },
    })
}

/// Load a fingerprint from disk
pub fn load_fingerprint(path: &Path) -> Option<DocFingerprint> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save a fingerprint to disk
pub fn save_fingerprint(
    path: &Path,
    fingerprint: &DocFingerprint,
) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(fingerprint)?;
    fs::write(path, content)?;
    Ok(())
}

/// Get the hash of the rustc version
fn get_rustc_version_hash() -> Result<u64, Box<dyn std::error::Error>> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()?;

    if !output.status.success() {
        return Err("Failed to get rustc version".into());
    }

    let version_string = String::from_utf8(output.stdout)?;
    let mut hasher = DefaultHasher::new();
    version_string.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Hash a single file's contents
fn hash_file(path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Recursively hash all Rust source files in a directory
fn hash_directory(dir: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let mut hasher = DefaultHasher::new();

    // Walk directory in sorted order for deterministic hashing
    let mut entries: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
        .collect();

    entries.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in entries {
        let path = entry.path();

        // Hash the relative path (so fingerprint survives project moves)
        if let Ok(rel_path) = path.strip_prefix(dir) {
            rel_path.to_string_lossy().hash(&mut hasher);
        }

        // Hash the file contents
        if let Ok(content) = fs::read_to_string(path) {
            content.hash(&mut hasher);
        }
    }

    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rustc_version_hash() {
        let hash = get_rustc_version_hash().expect("Failed to get rustc version");
        assert!(hash > 0);
    }
}
