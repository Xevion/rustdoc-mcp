mod common;

use assert2::{check, let_assert};
use common::{IsolatedWorkspace, isolated_workspace, isolated_workspace_with_serde};
use rstest::rstest;
use rustdoc_mcp::tools::inspect_item::{InspectItemRequest, handle_inspect_item};
use rustdoc_mcp::{DetailLevel, ItemKind};

/// Test: Find Serialize trait via serde::Serialize (resolves cross-crate re-exports).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_finds_serialize_trait(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde::Serialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Serialize"));
    check!(output.contains("trait"));
}

/// Test: Find Deserialize trait via simple lookup.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_successful_simple_lookup(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
}

/// Test: Find Serialize trait via qualified path.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_successful_qualified_path(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde::Serialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Serialize"));
    check!(output.contains("trait"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_no_matches_found(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "NonExistentItemXYZ123".to_string(),
        kind: None,
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    let_assert!(Err(err) = result);
    check!(err.contains("No items found matching"));
    check!(err.contains("NonExistentItemXYZ123"));
}

/// Test: Inspect with minimal verbosity.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_minimal_verbosity(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Low,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    check!(output.lines().count() < 20);
}

/// Test: Inspect with full verbosity.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_full_verbosity(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    check!(output.lines().count() >= 7);
    check!(output.contains("Methods:"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_function_lookup(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde_json::to_string".to_string(),
        kind: Some(ItemKind::Function),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("to_string"));
    check!(output.contains("fn"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_enum_with_variants(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "serde_json::Value".to_string(),
        kind: Some(ItemKind::Enum),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_item(&isolated_workspace_with_serde.context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Value"));
    check!(output.contains("enum"));
    check!(output.contains("Null") || output.contains("Bool") || output.contains("Number"));
}

/// Test: Find a local struct by simple name.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_struct_simple_name(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "QueryContext".to_string(),
        kind: Some(ItemKind::Struct),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(result.is_ok(), "Should find QueryContext by simple name");

    let output = result.unwrap();
    check!(output.contains("QueryContext"));
    check!(output.contains("struct"));

    // The path should NOT have a doubled crate name
    check!(
        !output.contains("rustdoc-mcp::rustdoc_mcp"),
        "Path should not have doubled crate name prefix"
    );
}

/// Test: Find a local struct by full qualified path.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_struct_full_path(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "rustdoc_mcp::search::query::QueryContext".to_string(),
        kind: Some(ItemKind::Struct),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(
        result.is_ok(),
        "Should find QueryContext by full path: {:?}",
        result
    );

    let output = result.unwrap();
    check!(output.contains("QueryContext"));
    check!(output.contains("struct"));
}

/// Test: Find a local module by name.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_module(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "workspace".to_string(),
        kind: Some(ItemKind::Module),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(result.is_ok(), "Should find workspace module: {:?}", result);

    let output = result.unwrap();
    check!(output.contains("workspace"));
}

/// Test: Find the TypeFormatter trait in the local crate.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_trait(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "TypeFormatter".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(
        result.is_ok(),
        "Should find TypeFormatter trait: {:?}",
        result
    );

    let output = result.unwrap();
    check!(output.contains("TypeFormatter"));
    check!(output.contains("trait"));
}

/// Test: Find BackgroundWorker struct (public export).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_backgroundworker(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "BackgroundWorker".to_string(),
        kind: Some(ItemKind::Struct),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(
        result.is_ok(),
        "Should find BackgroundWorker struct: {:?}",
        result
    );

    let output = result.unwrap();
    check!(output.contains("BackgroundWorker"));
    check!(output.contains("struct"));
}

/// Test: Find ServerContext with full path using hyphenated crate name.
/// This tests if the crate name normalization works correctly.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn inspect_local_with_hyphenated_crate_name(isolated_workspace: IsolatedWorkspace) {
    let request = InspectItemRequest {
        query: "rustdoc-mcp::ServerContext".to_string(),
        kind: Some(ItemKind::Struct),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&isolated_workspace.context, request).await;
    check!(
        result.is_ok(),
        "Should find ServerContext with hyphenated crate name: {:?}",
        result
    );

    let output = result.unwrap();
    check!(output.contains("ServerContext"));
}
