//! Workspace context and crate metadata types.

use crate::types::CrateName;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Type of crate in the workspace context
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrateOrigin {
    /// A workspace member (local crate)
    Local,
    /// An external library dependency
    External,
    /// A Rust standard library crate (std, core, alloc, etc.)
    Standard,
}

/// Metadata about a specific crate.
#[derive(Debug, Clone)]
pub struct CrateMetadata {
    /// Type of crate
    pub origin: CrateOrigin,
    /// Version string (if known)
    pub version: Option<String>,
    /// Description from Cargo.toml (if available)
    pub description: Option<String>,
    /// Is this a dev dependency?
    pub dev_dep: bool,
    /// Crate name
    pub name: CrateName,
    /// Is this the default crate (root crate)?
    pub is_root_crate: bool,
    /// Which workspace members use this dependency
    pub used_by: Vec<CrateName>,
}

/// Context about a Rust workspace discovered via cargo metadata.
///
/// Contains workspace members, dependencies, and their resolved versions.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Workspace root path
    pub root: PathBuf,

    /// Workspace members (crate names)
    pub members: Vec<CrateName>,

    /// Detailed crate information with usage tracking, indexed by crate name
    pub crate_info: HashMap<CrateName, CrateMetadata>,

    /// Root crate name (if this is a single-crate workspace)
    pub root_crate: Option<CrateName>,
}

impl WorkspaceContext {
    /// Get the default crate name (root crate or first workspace member).
    pub fn default_crate_name(&self) -> Option<&CrateName> {
        self.root_crate.as_ref().or_else(|| self.members.first())
    }

    /// Detect if we're in a subcrate context (working directory is a workspace member).
    pub fn detect_subcrate_context(&self) -> Option<&str> {
        // Check if root_crate is one of the workspace members
        if let Some(root_pkg) = &self.root_crate
            && self.members.len() > 1
            && self.members.contains(root_pkg)
        {
            return Some(root_pkg.as_str());
        }
        None
    }

    /// Get the version of a crate by name.
    pub fn get_version(&self, name: &str) -> Option<&str> {
        let key = CrateName::new_unchecked(name);
        self.crate_info.get(&key).and_then(|m| m.version.as_deref())
    }

    /// Get an iterator over dependency names (excludes workspace members).
    pub fn dependency_names(&self) -> impl Iterator<Item = &str> {
        self.crate_info
            .keys()
            .filter(|name| !self.members.iter().any(|m| m.matches(name.as_str())))
            .map(|s| s.as_str())
    }

    /// Get metadata for a specific crate by name.
    pub fn get_crate(&self, name: &str) -> Option<&CrateMetadata> {
        let key = CrateName::new_unchecked(name);
        self.crate_info.get(&key)
    }

    /// Get an iterator over crate info, optionally filtered by workspace member.
    pub fn iter_crates(&self, member_name: Option<&str>) -> impl Iterator<Item = &CrateMetadata> {
        let filter_member = member_name.or_else(|| self.detect_subcrate_context());
        let member_string = filter_member.map(|s| s.to_string());

        self.crate_info.values().filter(move |info| {
            match &member_string {
                Some(member) => {
                    // Include: workspace members + deps used by this member + standard library
                    info.origin == CrateOrigin::Local
                        || info.used_by.iter().any(|m| m.matches(member))
                        || info.origin == CrateOrigin::Standard
                }
                None => true, // Include all for workspace view
            }
        })
    }

    /// Get crates sorted by priority for documentation generation.
    ///
    /// Priority order:
    /// 1. Root crate (user's own code, most likely to be queried first)
    /// 2. Direct dependencies sorted by usage count (most-used first)
    /// 3. Dev dependencies (lower priority)
    /// 4. Transitive dependencies (rarely queried directly)
    pub fn prioritized_crates(&self) -> Vec<CrateName> {
        let mut crates: Vec<_> = self
            .crate_info
            .values()
            .filter(|c| c.origin != CrateOrigin::Standard) // Skip stdlib
            .collect();

        crates.sort_by(|a, b| {
            // Root crate first
            b.is_root_crate
                .cmp(&a.is_root_crate)
                // Then workspace members
                .then_with(|| {
                    let a_local = a.origin == CrateOrigin::Local;
                    let b_local = b.origin == CrateOrigin::Local;
                    b_local.cmp(&a_local)
                })
                // Then by usage count (most-used first)
                .then_with(|| b.used_by.len().cmp(&a.used_by.len()))
                // Non-dev deps before dev deps
                .then_with(|| a.dev_dep.cmp(&b.dev_dep))
                // Finally alphabetical for stability
                .then_with(|| a.name.normalized().cmp(b.name.normalized()))
        });

        crates.into_iter().map(|c| c.name.clone()).collect()
    }
}
