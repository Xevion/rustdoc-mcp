mod common;

use common::TempWorkspace;
use rustdoc_mcp::workspace::{
    find_cargo_toml_with_constraints, find_git_root, find_workspace_root, has_workspace_section,
    is_boundary_directory, is_system_directory,
};
use std::path::Path;

#[test]
fn test_find_cargo_toml_in_current_directory() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);

    let result = find_cargo_toml_with_constraints(workspace.path());
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("Cargo.toml"));
}

#[test]
fn test_find_cargo_toml_one_directory_up() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);
    workspace.create_dir("subdir");

    let result = find_cargo_toml_with_constraints(&workspace.path().join("subdir"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("Cargo.toml"));
}

#[test]
fn test_find_cargo_toml_two_directories_up_no_git() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);
    workspace.create_dir("dir1/dir2");

    let result = find_cargo_toml_with_constraints(&workspace.path().join("dir1/dir2"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("Cargo.toml"));
}

#[test]
fn test_stop_after_two_directories_no_git() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);
    workspace.create_dir("dir1/dir2/dir3");

    // Starting from dir3, can only go up 2 dirs (to dir1), won't find Cargo.toml at root
    let result = find_cargo_toml_with_constraints(&workspace.path().join("dir1/dir2/dir3"));
    assert!(result.is_none());
}

#[test]
fn test_unlimited_depth_in_git_repo() {
    let workspace = TempWorkspace::new();
    workspace.create_git_repo(".");
    workspace.create_cargo_toml("Cargo.toml", false);
    workspace.create_dir("dir1/dir2/dir3/dir4");

    // In a Git repo, can search unlimited depth
    let result = find_cargo_toml_with_constraints(&workspace.path().join("dir1/dir2/dir3/dir4"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("Cargo.toml"));
}

#[test]
fn test_stop_at_git_repository_root() {
    let workspace = TempWorkspace::new();
    workspace.create_dir("parent");
    workspace.create_cargo_toml("parent/Cargo.toml", false);
    workspace.create_dir("parent/repo");
    workspace.create_git_repo("parent/repo");
    workspace.create_cargo_toml("parent/repo/Cargo.toml", true);
    workspace.create_dir("parent/repo/subdir");

    // Should find repo/Cargo.toml, not parent/Cargo.toml
    let result = find_cargo_toml_with_constraints(&workspace.path().join("parent/repo/subdir"));
    assert!(result.is_some());
    let found = result.unwrap();
    assert_eq!(found, workspace.path().join("parent/repo/Cargo.toml"));
}

#[test]
fn test_git_submodule_boundary() {
    let workspace = TempWorkspace::new();
    workspace.create_dir("parent");
    workspace.create_git_repo("parent");
    workspace.create_cargo_toml("parent/Cargo.toml", false);
    workspace.create_dir("parent/submodule");
    workspace.create_git_submodule("parent/submodule");
    workspace.create_cargo_toml("parent/submodule/Cargo.toml", true);
    workspace.create_dir("parent/submodule/deep");

    // Should find submodule/Cargo.toml, not exit to parent
    let result = find_cargo_toml_with_constraints(&workspace.path().join("parent/submodule/deep"));
    assert!(result.is_some());
    let found = result.unwrap();
    assert_eq!(found, workspace.path().join("parent/submodule/Cargo.toml"));
}

#[test]
fn test_find_git_root_in_repo() {
    let workspace = TempWorkspace::new();
    workspace.create_git_repo(".");
    workspace.create_dir("deep/nested/path");

    let result = find_git_root(&workspace.path().join("deep/nested/path"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path());
}

#[test]
fn test_find_git_root_not_in_repo() {
    let workspace = TempWorkspace::new();
    workspace.create_dir("no/git/here");

    let result = find_git_root(&workspace.path().join("no/git/here"));
    assert!(result.is_none());
}

#[test]
fn test_find_git_root_with_submodule() {
    let workspace = TempWorkspace::new();
    workspace.create_git_repo(".");
    workspace.create_dir("submodule");
    workspace.create_git_submodule("submodule");
    workspace.create_dir("submodule/nested");

    // Should find submodule as the git root (innermost .git)
    let result = find_git_root(&workspace.path().join("submodule/nested"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("submodule"));
}

#[test]
fn test_has_workspace_section_workspace() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", true);

    let result = has_workspace_section(&workspace.path().join("Cargo.toml"));
    assert_eq!(result, Some(true));
}

#[test]
fn test_has_workspace_section_package_only() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);

    let result = has_workspace_section(&workspace.path().join("Cargo.toml"));
    assert_eq!(result, Some(false));
}

#[test]
fn test_has_workspace_section_invalid() {
    let workspace = TempWorkspace::new();
    workspace.create_file("Cargo.toml", "invalid toml content {][}");

    let result = has_workspace_section(&workspace.path().join("Cargo.toml"));
    assert_eq!(result, None);
}

#[test]
fn test_has_workspace_section_nonexistent() {
    let workspace = TempWorkspace::new();

    let result = has_workspace_section(&workspace.path().join("nonexistent.toml"));
    assert_eq!(result, None);
}

#[test]
fn test_find_workspace_root_already_workspace() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", true);

    let result = find_workspace_root(workspace.path());
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path());
}

#[test]
fn test_find_workspace_root_from_package() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", true);
    workspace.create_dir("member");
    workspace.create_cargo_toml("member/Cargo.toml", false);

    let result = find_workspace_root(&workspace.path().join("member"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path());
}

#[test]
fn test_find_workspace_root_nested_packages() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", true);
    workspace.create_dir("member1");
    workspace.create_cargo_toml("member1/Cargo.toml", false);
    workspace.create_dir("member1/nested");
    workspace.create_cargo_toml("member1/nested/Cargo.toml", false);

    // Should walk up past both packages to find workspace
    let result = find_workspace_root(&workspace.path().join("member1/nested"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path());
}

#[test]
fn test_find_workspace_root_package_without_workspace() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", false);

    // No workspace found, should return the package directory itself
    let result = find_workspace_root(workspace.path());
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path());
}

#[test]
fn test_is_system_directory_unix_system_dirs() {
    if cfg!(unix) {
        assert!(is_system_directory(Path::new("/usr")));
        assert!(is_system_directory(Path::new("/usr/local")));
        assert!(is_system_directory(Path::new("/etc")));
        assert!(is_system_directory(Path::new("/var/log")));
        assert!(is_system_directory(Path::new("/opt")));
        assert!(is_system_directory(Path::new("/bin")));
        assert!(is_system_directory(Path::new("/sbin")));
        assert!(is_system_directory(Path::new("/lib")));
        assert!(is_system_directory(Path::new("/lib64")));
    }
}

#[test]
fn test_is_system_directory_unix_user_dirs() {
    if cfg!(unix) {
        assert!(!is_system_directory(Path::new("/home")));
        assert!(!is_system_directory(Path::new("/home/user")));
        assert!(!is_system_directory(Path::new("/home/user/projects")));
        assert!(!is_system_directory(Path::new("/Users/user")));
    }
}

#[test]
fn test_is_boundary_directory_system_root() {
    if cfg!(unix) {
        assert!(is_boundary_directory(Path::new("/")));
    }
}

#[test]
fn test_is_boundary_directory_system_dirs() {
    if cfg!(unix) {
        assert!(is_boundary_directory(Path::new("/usr")));
        assert!(is_boundary_directory(Path::new("/etc")));
        assert!(is_boundary_directory(Path::new("/var")));
    }
}

#[test]
fn test_is_boundary_directory_user_dirs() {
    if cfg!(unix) {
        assert!(!is_boundary_directory(Path::new("/home")));
        assert!(!is_boundary_directory(Path::new("/home/user")));
        assert!(!is_boundary_directory(Path::new("/home/user/code")));
    }
}

#[test]
fn test_no_cargo_toml_found() {
    let workspace = TempWorkspace::new();
    workspace.create_dir("empty/project");

    let result = find_cargo_toml_with_constraints(&workspace.path().join("empty/project"));
    assert!(result.is_none());
}

#[test]
fn test_cargo_toml_with_invalid_content() {
    let workspace = TempWorkspace::new();
    workspace.create_file("Cargo.toml", "invalid { toml ][");
    workspace.create_dir("subdir");

    // Should still find the Cargo.toml file (validation happens later)
    let result = find_cargo_toml_with_constraints(&workspace.path().join("subdir"));
    assert!(result.is_some());
}

#[test]
fn test_multiple_cargo_toml_finds_nearest() {
    let workspace = TempWorkspace::new();
    workspace.create_cargo_toml("Cargo.toml", true);
    workspace.create_dir("nested");
    workspace.create_cargo_toml("nested/Cargo.toml", false);
    workspace.create_dir("nested/deep");

    // Should find nested/Cargo.toml (nearest one)
    let result = find_cargo_toml_with_constraints(&workspace.path().join("nested/deep"));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), workspace.path().join("nested/Cargo.toml"));
}
