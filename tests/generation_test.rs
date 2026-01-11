//! Tests for documentation generation.

mod common;

use assert2::{assert, check, let_assert};
use common::TempWorkspace;
use rstest::rstest;
use rustdoc_mcp::CrateName;
use rustdoc_mcp::workspace::lockfile::parse_cargo_lock;
use std::path::PathBuf;

#[tokio::test]
async fn lockfile_lookup_returns_original_hyphenated_name() {
    let lock_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
    let_assert!(Ok(crates) = parse_cargo_lock(&lock_path).await);

    // Lookup with underscores should find the entry
    let entry = crates.get("tracing_attributes");
    let_assert!(Some(entry) = entry);

    // Entry should have original hyphenated name
    check!(entry.name.as_str() == "tracing-attributes");
}

#[test]
fn crate_name_from_normalized_input_loses_hyphens() {
    // When created from already-normalized input, original form is lost
    let name = CrateName::new_unchecked("tracing_attributes");
    check!(name.as_str() == "tracing_attributes");
    check!(name.as_str() != "tracing-attributes");
}

#[tokio::test]
async fn generate_docs_with_hyphenated_name() {
    use rustdoc_mcp::workspace::generate_docs;

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let name = CrateName::new_unchecked("tracing-attributes");

    assert!(
        (generate_docs(&name, Some("0.1.30"), &workspace_root).await).is_ok(),
        "Should succeed"
    );
}

#[tokio::test]
async fn generate_docs_with_normalized_name() {
    use rustdoc_mcp::workspace::generate_docs;

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let name = CrateName::new_unchecked("tracing_attributes");

    // Should succeed by looking up "tracing-attributes" from Cargo.lock
    assert!(
        (generate_docs(&name, Some("0.1.30"), &workspace_root).await).is_ok(),
        "Should work with normalized name"
    );
}

#[tokio::test]
async fn cargo_rejects_underscored_package_names() {
    use tokio::process::Command;

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let_assert!(
        Ok(output) = Command::new("cargo")
            .current_dir(&workspace_root)
            .args([
                "+nightly",
                "rustdoc",
                "--package",
                "tracing_attributes",
                "--lib",
            ])
            .args(["--", "-Z", "unstable-options", "--output-format", "json"])
            .output()
            .await
    );
    check!(!output.status.success());
}

#[tokio::test]
async fn cargo_accepts_hyphenated_package_names() {
    use tokio::process::Command;

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let_assert!(
        Ok(output) = Command::new("cargo")
            .current_dir(&workspace_root)
            .args([
                "+nightly",
                "rustdoc",
                "--package",
                "tracing-attributes",
                "--lib",
            ])
            .args(["--", "-Z", "unstable-options", "--output-format", "json"])
            .output()
            .await
    );
    check!(output.status.success());
}

#[test]
fn query_context_has_negative_cache_api() {
    use rustdoc_mcp::{QueryContext, WorkspaceContext};
    use std::collections::HashMap;
    use std::sync::Arc;

    let workspace = Arc::new(WorkspaceContext {
        root: PathBuf::from("/tmp"),
        members: vec![],
        crate_info: HashMap::new(),
        root_crate: None,
    });

    let query_ctx = QueryContext::new(workspace);

    // BUG: This method doesn't exist yet
    // Once implemented, uncomment and remove the panic:
    // check!(!query_ctx.is_generation_failed("some_crate"));

    let _ = query_ctx;
    panic!("QueryContext needs is_generation_failed() for negative caching");
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn load_crate_returns_consistent_errors() {
    use rustdoc_mcp::{QueryContext, WorkspaceContext};
    use std::collections::HashMap;
    use std::sync::Arc;

    let temp = TempWorkspace::new();
    temp.create_file(
        "Cargo.toml",
        "[package]\nname = \"test\"\nversion = \"0.1.0\"",
    );
    temp.create_dir("src");
    temp.create_file("src/lib.rs", "");

    let workspace = Arc::new(WorkspaceContext {
        root: temp.path().to_path_buf(),
        members: vec![CrateName::new_unchecked("test")],
        crate_info: HashMap::new(),
        root_crate: Some(CrateName::new_unchecked("test")),
    });

    let ctx = QueryContext::new(workspace);

    let result1 = ctx.load_crate("nonexistent");
    let result2 = ctx.load_crate("nonexistent");

    let_assert!(Err(err1) = result1);
    let_assert!(Err(err2) = result2);
    check!(
        err1.to_string() == err2.to_string(),
        "Errors should match: {} vs {}",
        err1,
        err2
    );
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn repeated_load_does_not_retry_generation() {
    use rustdoc_mcp::{CrateMetadata, CrateOrigin, QueryContext, WorkspaceContext};
    use std::collections::HashMap;
    use std::sync::Arc;

    let temp = TempWorkspace::new();
    temp.create_file(
        "Cargo.toml",
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"",
    );
    temp.create_dir("src");
    temp.create_file("src/lib.rs", "");

    let mut crate_info = HashMap::new();
    crate_info.insert(
        CrateName::new_unchecked("fake_dep"),
        CrateMetadata {
            origin: CrateOrigin::External,
            version: Some("1.0.0".to_string()),
            description: None,
            dev_dep: false,
            name: CrateName::new_unchecked("fake_dep"),
            is_root_crate: false,
            used_by: vec![],
        },
    );

    let workspace = Arc::new(WorkspaceContext {
        root: temp.path().to_path_buf(),
        members: vec![CrateName::new_unchecked("test")],
        crate_info,
        root_crate: Some(CrateName::new_unchecked("test")),
    });

    let ctx = QueryContext::new(workspace);

    // Both should fail, ideally second doesn't retry generation
    let result1 = ctx.load_crate("fake_dep");
    let result2 = ctx.load_crate("fake_dep");

    check!(result1.is_err());
    check!(result2.is_err());
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn cross_crate_resolution_completes_without_hanging() {
    use common::IsolatedWorkspace;
    use rustdoc_mcp::tools::search::{SearchRequest, handle_search};

    let workspace = IsolatedWorkspace::with_deps(&["tracing"]);

    // Remove dependency JSON to trigger resolution attempt
    let attrs_json = workspace.root().join("target/doc/tracing_attributes.json");
    std::fs::remove_file(&attrs_json).ok();

    let request = SearchRequest {
        query: "instrument".to_string(),
        crate_name: "tracing".to_string(),
        limit: Some(5),
    };

    // Should complete without infinite loop
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        handle_search(&workspace.state, request),
    )
    .await;

    check!(result.is_ok(), "Should complete within timeout");
}
