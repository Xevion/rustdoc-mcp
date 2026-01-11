//! Cargo metadata execution and dependency resolution.

use crate::error::Result;

/// Validate version string matches semver format
pub(crate) fn validate_version(version: &str) -> Result<()> {
    let version_regex = regex::Regex::new(r"^\d+(\.\d+){0,2}").unwrap();
    if !version_regex.is_match(version) {
        anyhow::bail!(
            "Invalid version '{}': must be in semver format (e.g., 1.0.0)",
            version
        );
    }
    Ok(())
}
