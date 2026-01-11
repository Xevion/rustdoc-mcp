//! Rust workspace interaction: metadata, lockfiles, and documentation generation.

pub(crate) mod context;
pub(crate) mod detection;
pub mod lockfile;
pub(crate) mod metadata;
pub(crate) mod rustdoc;

pub use context::{CrateMetadata, CrateOrigin, WorkspaceContext};
pub use detection::{
    find_cargo_toml_with_constraints, find_git_root, find_workspace_root, has_workspace_section,
    is_boundary_directory, is_system_directory,
};
pub use rustdoc::generate_docs;

// Internal re-exports
pub(crate) use detection::{auto_detect_workspace, expand_tilde};
pub(crate) use rustdoc::get_docs;
