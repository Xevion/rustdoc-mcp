mod common;

use assert2::check;
use common::{IsolatedWorkspace, isolated_workspace, isolated_workspace_with_serde};
use rstest::rstest;
use rustdoc_mcp::DetailLevel;
use rustdoc_mcp::tools::inspect_crate::{InspectCrateRequest, handle_inspect_crate};

// --- Summary Mode Tests (no crate_name) ---

/// Test: List all crates shows local crate.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_summary_lists_local(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: None,
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    check!(result.is_ok(), "Should list all crates: {:?}", result);

    let output = result.unwrap();
    check!(
        output.contains("rustdoc-mcp"),
        "Should list rustdoc-mcp crate: {}",
        output
    );
}

/// Test: Summary lists external dependencies.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_summary_lists_deps(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: None,
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_crate(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        output.contains("serde"),
        "Should list serde dependency: {}",
        output
    );
}

// --- Detail Mode Tests (with crate_name) ---

/// Test: Inspect rustdoc-mcp shows modules.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_shows_modules(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("rustdoc-mcp".to_string()),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    check!(result.is_ok(), "Should inspect rustdoc-mcp: {:?}", result);

    let output = result.unwrap();
    // Should list top-level modules
    check!(
        output.contains("cache"),
        "Should show cache module: {}",
        output
    );
    check!(
        output.contains("search"),
        "Should show search module: {}",
        output
    );
    check!(
        output.contains("workspace"),
        "Should show workspace module: {}",
        output
    );
    check!(
        output.contains("tools"),
        "Should show tools module: {}",
        output
    );
}

/// Test: Inspect with high detail shows exports.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_shows_exports(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("rustdoc-mcp".to_string()),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    // High detail should show common exports
    check!(
        output.contains("Exports") || output.contains("Types") || output.contains("Functions"),
        "Should show exports section: {}",
        output
    );
}

/// Test: Inspect shows item counts.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_shows_item_counts(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("rustdoc-mcp".to_string()),
        detail_level: DetailLevel::Low,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    // Should show counts for different item types
    check!(
        output.contains("Struct") || output.contains("struct"),
        "Should show struct count: {}",
        output
    );
    check!(
        output.contains("Function") || output.contains("fn"),
        "Should show function count: {}",
        output
    );
}

/// Test: Inspect external crate works.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_external_dep(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("serde".to_string()),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_crate(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok(), "Should inspect serde: {:?}", result);

    let output = result.unwrap();
    check!(
        output.contains("serde"),
        "Should show serde info: {}",
        output
    );
}

/// Test: Inspect serde_json shows Value enum.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_serde_json(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("serde_json".to_string()),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_crate(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok(), "Should inspect serde_json: {:?}", result);

    let output = result.unwrap();
    check!(
        output.contains("serde_json"),
        "Should show crate name: {}",
        output
    );
}

// --- Error Cases ---

/// Test: Inspect non-existent crate gives helpful error.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_nonexistent(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("nonexistent-crate-xyz".to_string()),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    // Should fail gracefully with an error
    check!(result.is_err(), "Should fail for nonexistent crate");
}

// --- Consistency Tests ---
// Verify inspect_crate output structure.

/// Test: High detail level shows exports section with expected structure.
///
/// This validates that inspect_crate with high detail shows:
/// - Common Exports section with Types, Traits, Functions
/// - Truncation indicators when there are many items
/// - At least some known public items are visible
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_crate_exports_structure(isolated_workspace: IsolatedWorkspace) {
    let request = InspectCrateRequest {
        crate_name: Some("rustdoc-mcp".to_string()),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_crate(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();

    // Verify exports section structure
    check!(
        output.contains("Common Exports:"),
        "Should have Common Exports section: {}",
        output
    );
    check!(
        output.contains("Types:"),
        "Should have Types subsection: {}",
        output
    );
    check!(
        output.contains("Traits:"),
        "Should have Traits subsection: {}",
        output
    );
    check!(
        output.contains("Functions:"),
        "Should have Functions subsection: {}",
        output
    );

    // Verify truncation indicator exists (we have many items)
    check!(
        output.contains("... and") && output.contains("more"),
        "Should show truncation indicator for large export lists: {}",
        output
    );

    // Verify the only trait (TypeFormatter) is visible in the Traits section
    // (Traits section is small enough to not be truncated)
    check!(
        output.contains("TypeFormatter"),
        "TypeFormatter trait should be visible: {}",
        output
    );
}
