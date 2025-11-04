use cargo_doc_mcp::context::{ServerContext, WorkspaceMetadata};
use cargo_doc_mcp::handlers::inspect_item::{execute_inspect_item, InspectItemRequest, VerbosityLevel};
use std::path::PathBuf;

/// Helper to create a test context with the project root as workspace
fn setup_test_context() -> ServerContext {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut context = ServerContext::new();
    context.set_working_directory(project_root.clone())
        .expect("Failed to set working directory");

    // Set workspace metadata with serde as a dependency
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

#[test]
fn test_inspect_serialize() {
    let context = setup_test_context();

    let request = InspectItemRequest {
        query: "Serialize".to_string(),
        kind: Some(cargo_doc_mcp::types::ItemKind::Trait),
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);
    let err = result.expect_err("Should return error for ambiguous query");
    assert!(err.contains("Multiple items found matching"));
}

#[test]
fn test_inspect_serialize_document_tuple_variant() {
    let context = setup_test_context();

    let request = InspectItemRequest {
        query: "SerializeDocumentTupleVariant".to_string(),
        kind: None,
        verbosity: VerbosityLevel::Brief,
    };

    let result = execute_inspect_item(&context, request);

    // This should fail because the item isn't found in the doc index
    let output = result.expect("Should find SerializeDocumentTupleVariant");
    assert!(output.contains("struct SerializeDocumentTupleVariant"));
}
