//! Rust workspace interaction: metadata, lockfiles, and documentation generation.

pub mod context;
pub mod detection;
pub mod lockfile;
pub mod metadata;
pub mod rustdoc;

pub use context::{CrateMetadata, CrateOrigin, WorkspaceContext};
pub use detection::{
    auto_detect_workspace, expand_tilde, find_cargo_toml_with_constraints, find_git_root,
    find_workspace_root, has_workspace_section, is_boundary_directory, is_system_directory,
};
pub use lockfile::{LockfileEntry, parse_cargo_lock};
pub use metadata::{extract_dependencies, get_resolved_versions, validate_version};
pub use rustdoc::{generate_docs, get_docs};
