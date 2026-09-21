mod common;

use assert2::{assert, check};
use common::{
    IsolatedWorkspace, isolated_workspace, isolated_workspace_with_anyhow,
    isolated_workspace_with_serde, search_result_names, warm_cache,
};
use rstest::rstest;
use rustdoc_mcp::DetailLevel;
use rustdoc_mcp::index_metrics;
use rustdoc_mcp::tools::inspect_item::{InspectItemRequest, handle_inspect_item};
use rustdoc_mcp::tools::search::{SearchRequest, handle_search};

/// Searching needs the term index, not the parsed docs behind it. Once the index
/// is in memory, a repeat query must not re-parse every crate's rustdoc JSON —
/// that parse is what keeps tens of megabytes resident.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn warm_search_does_not_reparse_crate_docs(isolated_workspace: IsolatedWorkspace) {
    let request = || SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: 5,
    };

    handle_search(&isolated_workspace.state, request())
        .await
        .expect("first search");
    let parses_after_first = index_metrics::doc_parses();

    handle_search(&isolated_workspace.state, request())
        .await
        .expect("second search");
    let parses_after_second = index_metrics::doc_parses();

    check!(
        parses_after_second == parses_after_first,
        "warm search re-parsed rustdoc JSON \
         (parses went from {parses_after_first} to {parses_after_second})"
    );
}

/// An unqualified query fans out over every crate in the workspace. Crates whose
/// index answers "no match" contribute nothing to render, so none of them should
/// have their rustdoc JSON parsed just to reach that conclusion.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn fanout_does_not_parse_crates_without_matches(
    isolated_workspace_with_serde: IsolatedWorkspace,
) {
    warm_cache(
        &isolated_workspace_with_serde.state,
        &["rustdoc-mcp", "serde", "serde_json", "serde_core"],
    )
    .await;

    let restarted = isolated_workspace_with_serde.restart().await;
    let parses_before = index_metrics::doc_parses();

    let outcome = handle_inspect_item(
        &restarted,
        InspectItemRequest {
            query: "qqqzzzxxxwvu".to_string(),
            kind: None,
            detail_level: DetailLevel::Low,
        },
    )
    .await;

    check!(outcome.is_err(), "expected no matches for a nonsense query");

    let parsed = index_metrics::doc_parses() - parses_before;
    check!(
        parsed == 0,
        "fan-out parsed {parsed} crates to answer a query no index matched"
    );
}

/// An exact-name query resolves to items in one crate. Rendering is what forces a
/// parse, so every other crate's low-relevance token hits must be discarded from
/// the index alone, before anything is rendered.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn exact_name_query_parses_only_the_matching_crate(
    isolated_workspace_with_serde: IsolatedWorkspace,
) {
    warm_cache(
        &isolated_workspace_with_serde.state,
        &["rustdoc-mcp", "serde", "serde_json", "serde_core"],
    )
    .await;

    let restarted = isolated_workspace_with_serde.restart().await;
    let parses_before = index_metrics::doc_parses();

    let output = handle_inspect_item(
        &restarted,
        InspectItemRequest {
            query: "QueryContext".to_string(),
            kind: None,
            detail_level: DetailLevel::Low,
        },
    )
    .await
    .expect("inspect_item after restart");

    check!(output.contains("QueryContext"), "lost the result: {output}");

    let parsed = index_metrics::doc_parses() - parses_before;
    check!(
        parsed == 1,
        "parsed {parsed} crates to answer a name that exists in one"
    );
}

/// A parsed index should outlive the request that built it. Re-reading every
/// crate's index from disk on each query is what makes repeat calls slow.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn repeated_search_does_not_reload_index_from_disk(isolated_workspace: IsolatedWorkspace) {
    let request = || SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: 5,
    };

    handle_search(&isolated_workspace.state, request())
        .await
        .expect("first search");
    let (_, loads_after_first) = index_metrics::snapshot();

    handle_search(&isolated_workspace.state, request())
        .await
        .expect("second search");
    let (_, loads_after_second) = index_metrics::snapshot();

    check!(
        loads_after_second == loads_after_first,
        "second search re-read the index from disk \
         (loads went from {loads_after_first} to {loads_after_second})"
    );
}

// --- Working Search Tests ---
// These items ARE indexed and should work.

/// Test: Search finds QueryContext struct.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_querycontext(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: 5,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace.state, request).await,
        "Search should succeed"
    );
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace_with_serde.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace_with_serde.state, request).await);
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
        limit: 5,
    };

    assert!(let Ok(output) = handle_search(&isolated_workspace_with_serde.state, request).await);
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
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_with_fresh_index_build(isolated_workspace: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "QueryContext".to_string(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: 5,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace.state, request).await,
        "Fresh index search should succeed"
    );
    check!(
        output.contains("QueryContext"),
        "Should find QueryContext with fresh index: {}",
        output
    );
}

/// Test: Verify isolated workspace has no pre-existing index files.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn isolated_workspace_has_no_cached_index(isolated_workspace: IsolatedWorkspace) {
    let index_path = isolated_workspace
        .root()
        .join("target/doc/rustdoc_mcp.index");
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
        limit: 5,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace.state, request).await,
        "Search should succeed with warm cache"
    );
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
        limit: 5,
    };

    // Should return Ok with a suggestion message, not an Err
    assert!(let Ok(output) = handle_search(&isolated_workspace.state, request).await);
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
        query: String::new(),
        crate_name: "rustdoc-mcp".to_string(),
        limit: 5,
    };

    // Empty query should not panic
    assert!((handle_search(&isolated_workspace.state, request).await).is_ok());
}

// --- Concurrency Tests ---
// Tests for race conditions in parallel search operations.

/// Test: Concurrent searches against the same crate don't interfere.
///
/// This verifies that multiple simultaneous searches can share the same
/// index without data corruption or race conditions.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_searches_same_crate(isolated_workspace: IsolatedWorkspace) {
    // Spawn multiple concurrent searches
    let mut handles = vec![];
    let queries = ["QueryContext", "ServerContext", "CrateOrigin", "DocState"];

    for query in queries {
        let context = isolated_workspace.state.clone();
        let query = query.to_string();
        handles.push(tokio::spawn(async move {
            let request = SearchRequest {
                query: query.clone(),
                crate_name: "rustdoc-mcp".to_string(),
                limit: 5,
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
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_cold_cache_searches(isolated_workspace: IsolatedWorkspace) {
    // Verify no index exists yet
    let index_path = isolated_workspace
        .root()
        .join("target/doc/rustdoc_mcp.index");
    check!(
        !index_path.exists(),
        "Should start with cold cache: {:?}",
        index_path
    );

    // Launch many concurrent searches simultaneously
    let mut handles = vec![];
    for i in 0..10 {
        let context = isolated_workspace.state.clone();
        handles.push(tokio::spawn(async move {
            let request = SearchRequest {
                query: "QueryContext".to_string(),
                crate_name: "rustdoc-mcp".to_string(),
                limit: 5,
            };
            let result = handle_search(&context, request).await;
            (i, result)
        }));
    }

    // All should succeed despite racing to build the index
    let mut success_count = 0;
    for handle in handles {
        let (i, result) = handle.await.expect("Task should not panic");
        if let Ok(output) = result {
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
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_mixed_operations(isolated_workspace: IsolatedWorkspace) {
    // Start cache warming and searches at the same time
    let warm_handle = {
        let context = isolated_workspace.state.clone();
        tokio::spawn(async move {
            warm_cache(&context, &["rustdoc-mcp"]).await;
        })
    };

    // Spawn all tasks eagerly (collect is required — we need the spawns to race
    // with cache warming below, not lazily when the iterator is drained).
    #[allow(clippy::needless_collect)]
    let search_handles: Vec<_> = (0..5)
        .map(|i| {
            let context = isolated_workspace.state.clone();
            tokio::spawn(async move {
                // Small delay to interleave with warming
                tokio::time::sleep(tokio::time::Duration::from_millis(i * 10)).await;
                let request = SearchRequest {
                    query: "ServerContext".to_string(),
                    crate_name: "rustdoc-mcp".to_string(),
                    limit: 5,
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

// --- Bug: External dependency search broken ---
// The search index should contain entries for external dependency crates like anyhow.
// Currently, search returns no results for anyhow despite inspect_crate showing 65+ items.

/// Test: Search finds Error struct in anyhow crate.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_anyhow_error(isolated_workspace_with_anyhow: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Error".to_string(),
        crate_name: "anyhow".to_string(),
        limit: 10,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace_with_anyhow.state, request).await,
        "Search in anyhow should succeed"
    );
    check!(
        !output.contains("No results found"),
        "Should find results for 'Error' in anyhow: {}",
        output
    );
    check!(
        output.contains("Error"),
        "Should find Error in anyhow results: {}",
        output
    );
}

/// Test: Search finds Context trait in anyhow crate.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_anyhow_context(isolated_workspace_with_anyhow: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Context".to_string(),
        crate_name: "anyhow".to_string(),
        limit: 10,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace_with_anyhow.state, request).await,
        "Search in anyhow should succeed"
    );
    check!(
        !output.contains("No results found"),
        "Should find results for 'Context' in anyhow: {}",
        output
    );
    check!(
        output.contains("Context"),
        "Should find Context trait in anyhow results: {}",
        output
    );
}

/// Test: Search finds Result type alias in anyhow crate.
#[rstest]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_anyhow_result(isolated_workspace_with_anyhow: IsolatedWorkspace) {
    let request = SearchRequest {
        query: "Result".to_string(),
        crate_name: "anyhow".to_string(),
        limit: 10,
    };

    assert!(let
        Ok(output) = handle_search(&isolated_workspace_with_anyhow.state, request).await,
        "Search in anyhow should succeed"
    );
    check!(
        !output.contains("No results found"),
        "Should find results for 'Result' in anyhow: {}",
        output
    );
    check!(
        output.contains("Result"),
        "Should find Result type alias in anyhow results: {}",
        output
    );
}

/// Every public type should be reachable by its own name. Items whose module is
/// not public are absent from rustdoc's module listings, so an index built by
/// walking those listings can only see what a public re-export happens to expose.
#[rstest]
#[case("QueryContext")]
#[case("TypeFormatter")]
#[case("CrateOrigin")]
#[case("CrateName")]
#[case("TraitIterator")]
#[case("MethodIterator")]
#[case("ChildrenBuilder")]
#[case("ChildIterator")]
#[case("ItemRef")]
#[tokio::test(flavor = "multi_thread")]
async fn search_finds_type_by_name(#[case] name: &str, isolated_workspace: IsolatedWorkspace) {
    let output = handle_search(
        &isolated_workspace.state,
        SearchRequest {
            query: name.to_string(),
            crate_name: "rustdoc-mcp".to_string(),
            limit: 20,
        },
    )
    .await
    .expect("search should succeed");

    let names = search_result_names(&output);
    check!(
        names.iter().any(|found| found == name),
        "'{name}' missing from results: {names:?}"
    );
}

/// Queries are lowercased before hashing, so case should not decide whether an
/// item is findable.
#[rstest]
#[case("QueryContext")]
#[case("querycontext")]
#[case("QUERYCONTEXT")]
#[case("queryContext")]
#[tokio::test(flavor = "multi_thread")]
async fn search_is_case_insensitive(#[case] query: &str, isolated_workspace: IsolatedWorkspace) {
    let output = handle_search(
        &isolated_workspace.state,
        SearchRequest {
            query: query.to_string(),
            crate_name: "rustdoc-mcp".to_string(),
            limit: 10,
        },
    )
    .await
    .expect("search should succeed");

    check!(
        search_result_names(&output)
            .iter()
            .any(|n| n == "QueryContext"),
        "'{query}' did not find QueryContext"
    );
}

/// Cargo accepts a crate by either spelling, and both name the same crate.
#[rstest]
#[case("rustdoc-mcp")]
#[case("rustdoc_mcp")]
#[tokio::test(flavor = "multi_thread")]
async fn search_accepts_either_crate_name_spelling(
    #[case] crate_name: &str,
    isolated_workspace: IsolatedWorkspace,
) {
    let output = handle_search(
        &isolated_workspace.state,
        SearchRequest {
            query: "QueryContext".to_string(),
            crate_name: crate_name.to_string(),
            limit: 5,
        },
    )
    .await
    .expect("search should succeed");

    check!(
        search_result_names(&output)
            .iter()
            .any(|n| n == "QueryContext"),
        "crate spelled '{crate_name}' found nothing"
    );
}

/// Degenerate queries and limits must not panic or return more than asked for.
#[rstest]
#[case("", 10)]
#[case("   ", 10)]
#[case("::", 10)]
#[case("::QueryContext", 10)]
#[case("QueryContext::", 10)]
#[case("a::b::c::d::e::f::g", 10)]
#[case("QueryContext QueryContext QueryContext", 10)]
#[case("QueryContext", 0)]
#[case("QueryContext", 1)]
#[case("QueryContext", usize::MAX)]
#[case("\u{5f15}\u{6570}", 10)]
#[case("-", 10)]
#[case("________", 10)]
#[tokio::test(flavor = "multi_thread")]
async fn search_survives_degenerate_input(
    #[case] query: &str,
    #[case] limit: usize,
    isolated_workspace: IsolatedWorkspace,
) {
    let outcome = handle_search(
        &isolated_workspace.state,
        SearchRequest {
            query: query.to_string(),
            crate_name: "rustdoc-mcp".to_string(),
            limit,
        },
    )
    .await;

    if let Ok(output) = outcome {
        check!(
            search_result_names(&output).len() <= limit,
            "returned more than the requested limit of {limit}"
        );
    }
}
