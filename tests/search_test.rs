mod common;

use assert2::check;
use common::{IsolatedWorkspace, isolated_workspace, isolated_workspace_with_serde, warm_cache};
use rstest::rstest;
use rustdoc_mcp::tools::search::{SearchRequest, handle_search};

// --- Working Search Tests ---
// These items ARE indexed and should work.

/// Test: Search finds QueryContext struct.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_querycontext(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok(), "Search should succeed: {:?}", result);

    let output = result.unwrap();
    check!(
        output.contains("QueryContext"),
        "Should find QueryContext in results: {}",
        output
    );
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found'"
    );
}

/// Test: Search finds ServerContext struct.
/// This is one of the items that currently works.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_servercontext(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "ServerContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        output.contains("ServerContext"),
        "Should find ServerContext: {}",
        output
    );
}

/// Test: Search finds CrateOrigin enum.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_crateorigin(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "CrateOrigin".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        output.contains("CrateOrigin"),
        "Should find CrateOrigin: {}",
        output
    );
}

/// Test: Search finds TraitIterator struct.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_traititerator(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "TraitIterator".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    // Must check for "No results" FIRST - the error message contains the search term
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("TraitIterator"),
        "Should find TraitIterator in results"
    );
}

/// Test: Search finds BackgroundWorker struct (public export).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_backgroundworker(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "BackgroundWorker".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    // Must check for "No results" FIRST - the error message contains the search term
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("BackgroundWorker"),
        "Should find BackgroundWorker in results"
    );
}

/// Test: Search finds TypeFormatter trait.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_typeformatter_trait(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "TypeFormatter".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("TypeFormatter"),
        "Should find TypeFormatter trait in results"
    );
}

/// Test: Search finds the 'cache' module.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_module_cache(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "cache".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("cache"),
        "Should find cache module in results"
    );
}

/// Test: Search finds ItemRef struct (public export).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_itemref(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "ItemRef".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(output.contains("ItemRef"), "Should find ItemRef in results");
}

/// Test: Search finds Serialize trait in serde (via cross-crate re-export resolution).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_serde_serialize(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Serialize".to_string(),
        crate_name: "serde".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("Serialize"),
        "Should find Serialize trait in serde"
    );
}

/// Test: Search finds Deserialize trait in serde (via cross-crate re-export resolution).
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_serde_deserialize(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Deserialize".to_string(),
        crate_name: "serde".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("Deserialize"),
        "Should find Deserialize trait in serde"
    );
}

/// Test: Search finds Deserializer trait in serde.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_serde_deserializer(isolated_workspace_with_serde: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Deserializer".to_string(),
        crate_name: "serde".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace_with_serde.state, request).await;
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should not say 'no results found': {}",
        output
    );
    check!(
        output.contains("Deserializer"),
        "Should find Deserializer in serde"
    );
}

// --- Cache Testing ---
// Tests for index build behavior with cold and warm caches.

/// Test: Search with fresh index build (no cache).
/// Uses isolated workspace to ensure no cached index exists.
#[tokio::test(flavor = "multi_thread")]
async fn search_with_fresh_index_build() {
    let workspace = IsolatedWorkspace::new();

    let request = SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&workspace.state, request).await;
    check!(
        result.is_ok(),
        "Fresh index search should succeed: {:?}",
        result
    );

    let output = result.unwrap();
    check!(
        output.contains("QueryContext"),
        "Should find QueryContext with fresh index: {}",
        output
    );
}

/// Test: Verify isolated workspace has no pre-existing index files.
#[tokio::test(flavor = "multi_thread")]
async fn isolated_workspace_has_no_cached_index() {
    let workspace = IsolatedWorkspace::new();

    let index_path = workspace.root().join("target/doc/rustdoc_mcp.index");
    check!(
        !index_path.exists(),
        "Isolated workspace should not have cached index: {:?}",
        index_path
    );
}

/// Test: Verify that search works correctly after warming cache.
///
/// This validates that warm cache behavior is correct.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_works_with_warm_cache(isolated_workspace: IsolatedWorkspace) {
    // Warm the cache first
    warm_cache(&isolated_workspace.state, &["rustdoc-mcp"]).await;

    // Now search should use cached index
    let request = SearchRequest {
        query: "ServerContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    check!(result.is_ok(), "Search should succeed with warm cache");

    let output = result.unwrap();
    check!(
        !output.contains("No results found"),
        "Should find results with warm cache: {}",
        output
    );
}

// --- Edge Cases ---

/// Test: Search for non-existent crate gives helpful error.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_nonexistent_crate_error(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "anything".to_string(),
        crate_name: "nonexistent-crate-xyz".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    // Should return Ok with a suggestion message, not an Err
    check!(result.is_ok());

    let output = result.unwrap();
    check!(
        output.contains("not found") || output.contains("Did you mean"),
        "Should give helpful error for nonexistent crate: {}",
        output
    );
}

/// Test: Empty query string behavior.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_empty_query(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: Some(5),
    };

    let result = handle_search(&isolated_workspace.state, request).await;
    // Empty query should not panic
    check!(result.is_ok());
}

// --- Concurrency Tests ---
// Tests for race conditions in parallel search operations.

/// Test: Concurrent searches against the same crate don't interfere.
///
/// This verifies that multiple simultaneous searches can share the same
/// index without data corruption or race conditions.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_searches_same_crate() {
    let workspace = IsolatedWorkspace::new();

    // Spawn multiple concurrent searches
    let mut handles = vec![];
    let queries = ["QueryContext", "ServerContext", "CrateOrigin", "DocState"];

    for query in queries {
        let context = workspace.state.clone();
        let query = query.to_string();
        handles.push(tokio::spawn(async move {
            let request = SearchRequest {
                query: query.clone(),
                crate_name: "rustdoc-mcp".to_string(),
                limit: Some(5),
            };
            let result = handle_search(&context, request).await;
            (query, result)
        }));
    }

    // All searches should succeed
    for handle in handles {
        let (query, result) = handle.await.expect("Task should not panic");
        check!(result.is_ok(), "Search for '{}' should succeed", query);
    }
}

/// Test: Concurrent cold-cache searches trigger parallel index builds.
///
/// This tests the scenario where multiple searches start before any index
/// is built, forcing concurrent index construction attempts.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_cold_cache_searches() {
    let workspace = IsolatedWorkspace::new();

    // Verify no index exists yet
    let index_path = workspace.root().join("target/doc/rustdoc_mcp.index");
    check!(
        !index_path.exists(),
        "Should start with cold cache: {:?}",
        index_path
    );

    // Launch many concurrent searches simultaneously
    let mut handles = vec![];
    for i in 0..10 {
        let context = workspace.state.clone();
        handles.push(tokio::spawn(async move {
            let request = SearchRequest {
                query: "QueryContext".to_string(),
                crate_name: "rustdoc-mcp".to_string(),
                limit: Some(5),
            };
            let result = handle_search(&context, request).await;
            (i, result)
        }));
    }

    // All should succeed despite racing to build the index
    let mut success_count = 0;
    for handle in handles {
        let (i, result) = handle.await.expect("Task should not panic");
        if result.is_ok() {
            let output = result.unwrap();
            check!(
                output.contains("QueryContext"),
                "Search {} should find QueryContext",
                i
            );
            success_count += 1;
        }
    }

    check!(
        success_count == 10,
        "All 10 concurrent searches should succeed, got {}",
        success_count
    );
}

/// Test: Mixed read/write operations don't cause index corruption.
///
/// This simulates a realistic workload where searches and cache warming
/// happen concurrently.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_mixed_operations() {
    let workspace = IsolatedWorkspace::new();

    // Start cache warming and searches at the same time
    let warm_handle = {
        let context = workspace.state.clone();
        tokio::spawn(async move {
            warm_cache(&context, &["rustdoc-mcp"]).await;
        })
    };

    let search_handles: Vec<_> = (0..5)
        .map(|i| {
            let context = workspace.state.clone();
            tokio::spawn(async move {
                // Small delay to interleave with warming
                tokio::time::sleep(tokio::time::Duration::from_millis(i * 10)).await;
                let request = SearchRequest {
                    query: "ServerContext".to_string(),
                    crate_name: "rustdoc-mcp".to_string(),
                    limit: Some(5),
                };
                handle_search(&context, request).await
            })
        })
        .collect();

    // Wait for all operations
    warm_handle.await.expect("Warming should not panic");

    for (i, handle) in search_handles.into_iter().enumerate() {
        let result = handle.await.expect("Search should not panic");
        check!(result.is_ok(), "Search {} should succeed: {:?}", i, result);
    }
}
