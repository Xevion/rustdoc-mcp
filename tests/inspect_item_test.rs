use assert2::check;
use cargo_doc_mcp::context::{ServerContext, WorkspaceMetadata};
use cargo_doc_mcp::handlers::inspect_item::{execute_inspect_item, InspectItemRequest, VerbosityLevel};
use cargo_doc_mcp::types::ItemKind;
use rstest::rstest;
use std::path::PathBuf;

fn test_context() -> ServerContext {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut context = ServerContext::new();
    context.set_working_directory(project_root.clone())
        .expect("Failed to set working directory");

    let metadata = WorkspaceMetadata {
        root: project_root,
        members: vec!["cargo-doc-mcp".to_string()],
        dependencies: vec![
            ("serde".to_string(), "1.0".to_string()),
            ("serde_json".to_string(), "1.0".to_string()),
        ],
    };
    context.set_workspace_metadata(metadata);

    context
}

#[rstest]
#[case("serde::Serialize", &["Multiple items found matching", "serde::", "Serialize"])]
#[case("Serialize", &["Multiple items found matching", "serde::"])]
fn inspect_finds_multiple_matching_traits(#[case] query: &str, #[case] expected: &[&str]) {
    let context = test_context();

    let request = InspectItemRequest {
        query: query.to_string(),
        kind: Some(ItemKind::Trait),
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_err());

    let err = result.unwrap_err();
    for expected_str in expected {
        check!(err.contains(expected_str));
    }
}

// Phase 1: Core happy paths

#[test]
fn inspect_successful_simple_lookup() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
}

#[test]
fn inspect_successful_qualified_path() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde::Serialize".to_string(),
        kind: Some(ItemKind::Trait),
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Serialize"));
    check!(output.contains("trait"));
}

#[test]
fn inspect_no_matches_found() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "NonExistentItemXYZ123".to_string(),
        kind: None,
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_err());

    let err = result.unwrap_err();
    check!(err.contains("No items found matching"));
    check!(err.contains("NonExistentItemXYZ123"));
}

// Phase 2: Verbosity levels

#[test]
fn inspect_minimal_verbosity() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        verbosity: VerbosityLevel::Minimal,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    // Minimal should be very short, just the signature
    check!(output.lines().count() < 20);
}

#[test]
fn inspect_full_verbosity() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde::Deserialize".to_string(),
        kind: Some(ItemKind::Trait),
        verbosity: VerbosityLevel::Full,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Deserialize"));
    check!(output.contains("trait"));
    // Full should include methods, making it longer
    check!(output.lines().count() > 10);
}

// Phase 3: Different item types

#[test]
fn inspect_function_lookup() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde_json::to_string".to_string(),
        kind: Some(ItemKind::Function),
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("to_string"));
    check!(output.contains("fn"));
}

#[test]
fn inspect_enum_with_variants() {
    let context = test_context();

    let request = InspectItemRequest {
        query: "serde_json::Value".to_string(),
        kind: Some(ItemKind::Enum),
        verbosity: VerbosityLevel::Full,
    };

    let result = execute_inspect_item(&context, request);
    check!(result.is_ok());

    let output = result.unwrap();
    check!(output.contains("Value"));
    check!(output.contains("enum"));
    // Full verbosity should show variants
    check!(output.contains("Null") || output.contains("Bool") || output.contains("Number"));
}
