//! Automatic workspace detection for MCP server startup.
//!
//! This module provides functionality to automatically detect a Rust workspace
//! by walking up the directory tree from the process's current working directory,
//! respecting Git repository boundaries and system directory constraints.

use std::env;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Automatically detect a workspace starting from the current working directory.
///
/// This function orchestrates the detection logic:
/// 1. Get the current working directory
/// 2. Walk up directories looking for Cargo.toml
/// 3. Apply constraints (Git boundaries, system dirs, max depth)
/// 4. Validate that we found a workspace root (not just a package)
///
/// Returns the canonicalized path to the workspace directory, or None if no valid workspace found.
pub(crate) async fn auto_detect_workspace() -> Option<PathBuf> {
    let cwd = match env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            debug!("Failed to get current working directory: {}", e);
            return None;
        }
    };

    debug!("Starting workspace auto-detection from: {}", cwd.display());

    // Find Cargo.toml with all constraints applied
    let cargo_toml_path = find_cargo_toml_with_constraints(&cwd)?;
    let workspace_dir = cargo_toml_path.parent()?.to_path_buf();

    debug!(
        "Found Cargo.toml at: {}, validating workspace...",
        cargo_toml_path.display()
    );

    // Ensure we have a workspace root, not just a package member
    let workspace_root = find_workspace_root(&workspace_dir)?;

    // Canonicalize the path for consistency
    match tokio::fs::canonicalize(&workspace_root).await {
        Ok(canonical) => {
            info!("✓ Auto-detected workspace: {}", canonical.display());
            Some(canonical)
        }
        Err(e) => {
            warn!(
                "Found workspace at {} but canonicalization failed: {}",
                workspace_root.display(),
                e
            );
            None
        }
    }
}

/// Find a Cargo.toml file walking up from the given path, respecting all constraints.
///
/// Constraints:
/// - Stop at system boundaries (/, /home, C:\, C:\Windows, etc.)
/// - Stop at Git repository root (don't exit the repo)
/// - Stop at Git submodule boundaries (don't exit submodule)
/// - If not in a Git repo, only search up to 2 directories
/// - Never use /Cargo.toml or C:\Cargo.toml (system root)
///
/// Returns the path to the Cargo.toml file, or None if not found.
pub fn find_cargo_toml_with_constraints(start: &Path) -> Option<PathBuf> {
    let git_root = find_git_root(start);
    let max_depth = if git_root.is_some() { None } else { Some(2) };

    if let Some(ref git_root) = git_root {
        debug!("Git repository detected at: {}", git_root.display());
    } else {
        debug!("Not in a Git repository, limiting search to 2 directories up");
    }

    let mut current = start.to_path_buf();
    let mut depth = 0;

    loop {
        // Check for Cargo.toml in current directory
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() && !is_at_system_root(&current) {
            debug!("Found Cargo.toml at: {}", cargo_toml.display());
            return Some(cargo_toml);
        }

        // Check stop conditions
        if is_boundary_directory(&current) {
            debug!("Hit boundary directory: {}", current.display());
            break;
        }

        // Check if we would exit the Git repository
        if let Some(ref git_root) = git_root
            && current == git_root.as_path()
        {
            debug!("Reached Git repository root, stopping search");
            break;
        }

        // Check depth limit (only if not in Git repo)
        if let Some(max) = max_depth
            && depth >= max
        {
            debug!("Reached maximum search depth of {} directories", max);
            break;
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) => {
                current = parent.to_path_buf();
                depth += 1;
            }
            None => {
                debug!("Reached filesystem root");
                break;
            }
        }
    }

    debug!("No Cargo.toml found within constraints");
    None
}

/// Find the root of a Git repository by looking for .git directory or file.
///
/// Walks up from the given path until a .git directory or file is found.
/// Stops at the first .git found (handles submodules correctly).
///
/// Returns the path to the directory containing .git, or None if not in a Git repo.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let git_dir = current.join(".git");
        if git_dir.exists() {
            return Some(current);
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

/// Check if the given path is at the system root (/ or C:\).
///
/// This prevents using /Cargo.toml or C:\Cargo.toml as a valid workspace.
fn is_at_system_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// Check if the given path is a boundary directory that should stop the search.
///
/// Boundary directories include:
/// - System roots: /, C:\, D:\, etc.
/// - System directories: /usr, /etc, /var, /opt, /srv, /bin, /sbin, /lib
/// - Windows system directories: C:\Windows, C:\Program Files, etc.
///
/// Note: We allow searching within user directories like /home/user/, C:\Users\user\
pub fn is_boundary_directory(path: &Path) -> bool {
    // Check if at filesystem root
    if is_at_system_root(path) {
        return true;
    }

    // Check for known system directories
    is_system_directory(path)
}

/// Check if the given path is a protected system directory.
///
/// System directories include common Unix/Linux/macOS/Windows system paths
/// where Cargo workspaces are unlikely to exist.
pub fn is_system_directory(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();

    // Unix/Linux system directories
    let unix_system_dirs = [
        "/usr", "/etc", "/var", "/opt", "/srv", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/dev",
        "/proc", "/sys", "/run",
    ];

    for sys_dir in &unix_system_dirs {
        if path_str == *sys_dir || path_str.starts_with(&format!("{}/", sys_dir)) {
            return true;
        }
    }

    // Windows system directories
    let windows_system_patterns = [
        ":\\windows",
        ":\\program files",
        ":\\program files (x86)",
        ":\\programdata",
        ":\\$",
    ];

    for pattern in &windows_system_patterns {
        if path_str.contains(pattern) {
            return true;
        }
    }

    false
}

/// Find the workspace root starting from a potential package directory.
///
/// If the given directory contains a Cargo.toml with [workspace], returns it immediately.
/// If it contains a Cargo.toml with [package] only, walks up to find a [workspace].
/// Stops when a workspace is found or no parent directory exists.
///
/// Returns the workspace root directory, or None if no valid workspace found.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    let mut last_valid_cargo_dir = None;

    loop {
        // Check for boundary before checking for Cargo.toml
        if is_boundary_directory(&current) {
            debug!(
                "Hit boundary directory during workspace search: {}",
                current.display()
            );
            break;
        }

        let cargo_toml = current.join("Cargo.toml");

        if cargo_toml.exists() {
            match has_workspace_section(&cargo_toml) {
                Some(true) => {
                    debug!(
                        "Found workspace root with [workspace] section: {}",
                        current.display()
                    );
                    return Some(current);
                }
                Some(false) => {
                    debug!(
                        "Found [package] without [workspace], continuing search upward: {}",
                        current.display()
                    );
                    // Remember this as a valid fallback
                    if last_valid_cargo_dir.is_none() {
                        last_valid_cargo_dir = Some(current.clone());
                    }
                }
                None => {
                    debug!(
                        "Failed to parse Cargo.toml at {}, skipping",
                        cargo_toml.display()
                    );
                }
            }
        }

        // Try parent directory
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => {
                debug!("Reached filesystem root without finding [workspace]");
                break;
            }
        }
    }

    // If we found any Cargo.toml during the search, use the last one found
    // Otherwise return the start directory
    last_valid_cargo_dir.or_else(|| {
        if start.join("Cargo.toml").exists() {
            Some(start.to_path_buf())
        } else {
            None
        }
    })
}

/// Check if a Cargo.toml file has a [workspace] section.
///
/// Returns:
/// - Some(true) if [workspace] section exists
/// - Some(false) if only [package] section exists
/// - None if file cannot be read or parsed
pub fn has_workspace_section(cargo_toml: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(cargo_toml).ok()?;
    let toml: toml::Value = toml::from_str(&content).ok()?;

    let has_workspace = toml.as_table()?.contains_key("workspace");

    Some(has_workspace)
}

/// Expand tilde (`~`) in paths to the user's home directory.
///
/// Examples:
/// - `~/projects/foo` becomes `/home/user/projects/foo`
/// - `~` becomes `/home/user`
/// - Other paths are returned unchanged
///
/// Returns `Cow::Borrowed` if no expansion needed, `Cow::Owned` if expanded.
pub(crate) fn expand_tilde(path: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return Cow::Owned(home.join(stripped).display().to_string());
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return Cow::Owned(home.display().to_string());
    }
    Cow::Borrowed(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_at_system_root() {
        // Unix root
        assert!(is_at_system_root(Path::new("/")));

        // Windows roots
        if cfg!(windows) {
            assert!(is_at_system_root(Path::new("C:\\")));
            assert!(is_at_system_root(Path::new("D:\\")));
        }

        // Not roots
        assert!(!is_at_system_root(Path::new("/home")));
        assert!(!is_at_system_root(Path::new("/home/user")));
        if cfg!(windows) {
            assert!(!is_at_system_root(Path::new("C:\\Users")));
        }
    }

    #[test]
    fn test_is_system_directory() {
        // Unix system directories
        assert!(is_system_directory(Path::new("/usr")));
        assert!(is_system_directory(Path::new("/usr/local")));
        assert!(is_system_directory(Path::new("/etc")));
        assert!(is_system_directory(Path::new("/etc/nginx")));
        assert!(is_system_directory(Path::new("/var")));
        assert!(is_system_directory(Path::new("/opt")));

        // Windows system directories
        if cfg!(windows) {
            assert!(is_system_directory(Path::new("C:\\Windows")));
            assert!(is_system_directory(Path::new("C:\\Windows\\System32")));
            assert!(is_system_directory(Path::new("C:\\Program Files")));
            assert!(is_system_directory(Path::new("C:\\Program Files (x86)")));
        }

        // Not system directories
        assert!(!is_system_directory(Path::new("/home")));
        assert!(!is_system_directory(Path::new("/home/user")));
        assert!(!is_system_directory(Path::new("/home/user/projects")));

        if cfg!(windows) {
            assert!(!is_system_directory(Path::new("C:\\Users")));
            assert!(!is_system_directory(Path::new("C:\\Users\\user")));
        }
    }

    #[test]
    fn test_is_boundary_directory() {
        // System roots are boundaries
        assert!(is_boundary_directory(Path::new("/")));

        // System directories are boundaries
        assert!(is_boundary_directory(Path::new("/usr")));
        assert!(is_boundary_directory(Path::new("/etc")));

        // User directories are not boundaries
        assert!(!is_boundary_directory(Path::new("/home")));
        assert!(!is_boundary_directory(Path::new("/home/user")));
        assert!(!is_boundary_directory(Path::new("/home/user/projects")));
    }
}
