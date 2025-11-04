use crate::doc::DocIndex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tracing::{debug, error, info};
use tracing_subscriber::fmt;

#[derive(Debug, Deserialize)]
pub struct CargoMetadata {
    pub packages: Vec<MetadataPackage>,
    pub resolve: Option<MetadataResolve>,
    pub workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MetadataPackage {
    pub name: String,
    pub version: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct MetadataResolve {
    pub nodes: Vec<MetadataNode>,
}

#[derive(Debug, Deserialize)]
pub struct MetadataNode {
    pub id: String,
    pub dependencies: Vec<String>,
}

pub struct UptimeTimer {
    start: Instant,
}

impl UptimeTimer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl fmt::time::FormatTime for UptimeTimer {
    fn format_time(&self, w: &mut fmt::format::Writer<'_>) -> std::fmt::Result {
        let elapsed = self.start.elapsed();
        let seconds = elapsed.as_secs_f64();
        write!(w, "{:7.3}", seconds)
    }
}

pub fn get_resolved_versions() -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version=1")
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to run cargo metadata: {}", stderr).into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let metadata: CargoMetadata = serde_json::from_str(&stdout)?;

    let mut packages_by_id: HashMap<String, &MetadataPackage> = HashMap::new();
    for package in &metadata.packages {
        packages_by_id.insert(package.id.clone(), package);
    }

    let workspace_roots: HashSet<&str> = metadata.workspace_members.iter()
        .map(|s| s.as_str())
        .collect();

    let mut direct_deps: HashMap<String, String> = HashMap::new();

    if let Some(resolve) = &metadata.resolve {
        for node in &resolve.nodes {
            if workspace_roots.contains(node.id.as_str()) {
                for dep_id in &node.dependencies {
                    if let Some(dep_pkg) = packages_by_id.get(dep_id) {
                        direct_deps.entry(dep_pkg.name.clone())
                            .or_insert(dep_pkg.version.clone());
                    }
                }
            }
        }
    }

    Ok(direct_deps)
}

pub fn get_docs(crate_name: &str, version: Option<&str>) -> Result<DocIndex, Box<dyn std::error::Error>> {
    let normalized_name = crate_name.replace('-', "_");
    let doc_path = format!("target/doc/{}.json", normalized_name);

    if !Path::new(&doc_path).exists() {
        debug!("Documentation not found at {}", doc_path);
        info!("Generating documentation for {}{}", crate_name,
            version.map(|v| format!("@{}", v)).unwrap_or_default());

        generate_docs(crate_name, version)?;

        info!("Documentation generated");
    }

    DocIndex::load(&doc_path)
}

pub fn generate_docs(crate_name: &str, version: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let package_spec = if let Some(ver) = version {
        format!("{}@{}", crate_name, ver)
    } else {
        crate_name.to_string()
    };

    let output = Command::new("cargo")
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
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("Failed to generate documentation for '{}': {}", package_spec, stderr);
        error!("Make sure: 1) Nightly toolchain is installed (rustup install nightly), 2) The crate exists in your dependencies");
        return Err(format!("rustdoc command failed for crate '{}'", package_spec).into());
    }

    Ok(())
}

pub fn find_cargo_toml() -> Option<PathBuf> {
    let mut current_dir = env::current_dir().ok()?;

    loop {
        let cargo_toml = current_dir.join("Cargo.toml");
        if cargo_toml.exists() {
            return Some(cargo_toml);
        }

        if !current_dir.pop() {
            return None;
        }
    }
}

pub fn extract_dependencies(cargo_toml_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(cargo_toml_path)?;
    let toml_value: toml::Value = toml::from_str(&content)?;

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
