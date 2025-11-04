use crate::doc::DocIndex;
use cargo_metadata::{DependencyKind, MetadataCommand};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tracing::{debug, error, info};
use tracing_subscriber::fmt;

/// Validate crate name contains only safe characters
fn validate_crate_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let crate_name_regex = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();
    if !crate_name_regex.is_match(name) {
        return Err(format!(
            "Invalid crate name '{}': must contain only alphanumeric characters, hyphens, and underscores",
            name
        ).into());
    }
    Ok(())
}

/// Validate version string matches semver format
fn validate_version(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    let version_regex = regex::Regex::new(r"^\d+(\.\d+){0,2}").unwrap();
    if !version_regex.is_match(version) {
        return Err(format!(
            "Invalid version '{}': must be in semver format (e.g., 1.0.0)",
            version
        ).into());
    }
    Ok(())
}

pub struct UptimeTimer {
    start: Instant,
}

impl Default for UptimeTimer {
    fn default() -> Self {
        Self::new()
    }
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
    let metadata = MetadataCommand::new()
        .exec()
        .map_err(|e| format!("Failed to run cargo metadata: {}", e))?;

    let mut direct_deps: HashMap<String, String> = HashMap::new();

    // Get all workspace package IDs
    let workspace_pkg_ids: HashSet<_> = metadata.workspace_members.iter().collect();

    // For each workspace package, collect its direct dependencies
    for pkg in &metadata.packages {
        if workspace_pkg_ids.contains(&pkg.id) {
            for dep in &pkg.dependencies {
                if dep.kind == DependencyKind::Normal {
                    // Find the resolved version from packages
                    if let Some(dep_pkg) = metadata.packages.iter()
                        .find(|p| p.name == dep.name) {
                        direct_deps.entry(dep_pkg.name.to_string())
                            .or_insert(dep_pkg.version.to_string());
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
