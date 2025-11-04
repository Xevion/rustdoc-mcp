use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Metadata for a package from Cargo.lock
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub checksum: Option<String>,
    pub source: Option<String>,
}

/// Parse Cargo.lock and return a map of package name to metadata
pub fn parse_cargo_lock(
    lock_path: &Path,
) -> Result<HashMap<String, PackageMetadata>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(lock_path)?;
    let lockfile: CargoLock = toml::from_str(&content)?;

    let mut packages = HashMap::new();

    for package in lockfile.package {
        packages.insert(
            package.name.clone(),
            PackageMetadata {
                name: package.name,
                version: package.version,
                checksum: package.checksum,
                source: package.source,
            },
        );
    }

    Ok(packages)
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
    use std::path::PathBuf;

    #[test]
    fn test_parse_cargo_lock() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let lock_path = manifest_dir.join("Cargo.lock");

        if lock_path.exists() {
            let packages = parse_cargo_lock(&lock_path).expect("Failed to parse Cargo.lock");
            assert!(!packages.is_empty());

            // Should have serde as a dependency
            assert!(packages.contains_key("serde"));

            // Check that serde has a checksum
            let serde = &packages["serde"];
            assert!(serde.checksum.is_some());
        }
    }
}
