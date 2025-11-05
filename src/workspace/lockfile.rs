//! Cargo.lock parsing and lockfile entry management.

use crate::cache::Hash;
use crate::error::Result;
use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Metadata for a crate entry from Cargo.lock
#[derive(Debug, Clone)]
pub struct LockfileEntry {
    pub name: String,
    pub version: String,
    pub checksum: Option<Hash>,
    pub source: Option<String>,
}

/// Parse Cargo.lock and return a map of crate name to lockfile entry
pub async fn parse_cargo_lock(lock_path: &Path) -> Result<HashMap<String, LockfileEntry>> {
    let content = tokio::fs::read_to_string(lock_path)
        .await
        .with_context(|| format!("Failed to read Cargo.lock at {}", lock_path.display()))?;
    let lockfile: CargoLock = toml::from_str(&content).context("Failed to parse Cargo.lock")?;

    let mut crates = HashMap::new();

    for package in lockfile.package {
        let checksum = match package.checksum {
            Some(ref checksum_str) => Some(checksum_str.parse::<Hash>().with_context(|| {
                format!(
                    "Invalid checksum '{}' for crate '{}'",
                    checksum_str, package.name
                )
            })?),
            None => None,
        };

        crates.insert(
            package.name.clone(),
            LockfileEntry {
                name: package.name,
                version: package.version,
                checksum,
                source: package.source,
            },
        );
    }

    Ok(crates)
}

#[derive(Debug, Deserialize)]
struct CargoLock {
    #[serde(default)]
    package: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use std::env;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_parse_cargo_lock() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let lock_path = manifest_dir.join("Cargo.lock");

        if lock_path.exists() {
            let crates = parse_cargo_lock(&lock_path)
                .await
                .expect("Failed to parse Cargo.lock");
            check!(!crates.is_empty());

            check!(crates.contains_key("serde"));

            let serde = &crates["serde"];
            check!(serde.checksum.is_some());
        }
    }
}
