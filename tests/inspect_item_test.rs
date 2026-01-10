use assert2::{check, let_assert};
use rstest::{fixture, rstest};
use rustdoc_mcp::tools::inspect_item::{InspectItemRequest, handle_inspect_item};
use rustdoc_mcp::{
    CrateMetadata, CrateOrigin, DetailLevel, ItemKind, ServerContext, WorkspaceContext,
};
use serial_test::serial;
use std::collections::HashMap;
use std::path::PathBuf;

#[fixture]
fn test_context() -> ServerContext {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut context = ServerContext::new();
    context
        .set_working_directory(project_root.clone())
        .expect("Failed to set working directory");

    let mut crate_info = HashMap::new();
    crate_info.insert(
        "serde".to_string(),
        CrateMetadata {
            origin: CrateOrigin::External,
            version: Some("1.0".to_string()),
            description: None,
            dev_dep: false,
            name: "serde".to_string(),
            is_root_crate: false,
            used_by: vec![],
        },
    );
    crate_info.insert(
        "serde_json".to_string(),
        CrateMetadata {
            origin: CrateOrigin::External,
            version: Some("1.0".to_string()),
            description: None,
            dev_dep: false,
            name: "serde_json".to_string(),
            is_root_crate: false,
            used_by: vec![],
        },
    );

    let metadata = WorkspaceContext {
        root: project_root,
        members: vec!["rustdoc-mcp".to_string()],
        crate_info,
        root_crate: Some("rustdoc-mcp".to_string()),
    };
    context.set_workspace_context(metadata);

    context
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_finds_serialize_trait(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde::Serialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Serialize"));
    check!(output.contains("trait"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_successful_simple_lookup(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_successful_qualified_path(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde::Serialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Serialize"));
    check!(output.contains("trait"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_no_matches_found(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "NonExistentItemXYZ123".to_string(),
        kind: None,
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&test_context, request).await;
    let_assert!(Err(err) = result);
    check!(err.contains("No items found matching"));
    check!(err.contains("NonExistentItemXYZ123"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_minimal_verbosity(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::Low,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    check!(output.lines().count() < 20);
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_full_verbosity(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    check!(output.lines().count() >= 7);
    check!(output.contains("Methods:"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_function_lookup(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde_json::to_string".to_string(),
        kind: Some(ItemKind::Function),
        detail_level: DetailLevel::Medium,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("to_string"));
    check!(output.contains("fn"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn inspect_enum_with_variants(test_context: ServerContext) {
    let request = InspectItemRequest {
        query: "serde_json::Value".to_string(),
        kind: Some(ItemKind::Enum),
        detail_level: DetailLevel::High,
    };

    let result = handle_inspect_item(&test_context, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Value"));
    check!(output.contains("enum"));
    check!(output.contains("Null") || output.contains("Bool") || output.contains("Number"));
}
