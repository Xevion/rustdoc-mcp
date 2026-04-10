//! Regression tests for stdlib search and inspection.
//!
//! These tests exercise the preloaded-crate path through `QueryContext` that
//! bridges `StdlibDocs` (sysroot-based) with the normal workspace-oriented
//! search infrastructure.
//!
//! # XEV-625
//!
//! Prior to the fix, stdlib queries would fail with "Failed to build search
//! index for std" because `handle_stdlib_search` / `handle_stdlib_inspect`
//! built a fake `WorkspaceContext` pointing at `/`, and `TermIndex` then
//! looked for docs at `/target/doc/std.json`. These tests lock down the
//! happy path for the stdlib crates we care about.
//!
//! # Test isolation
//!
//! Each test constructs its own `StdlibDocs` with a `TempDir` cache root via
//! `with_cache_root`. This gives each test a clean on-disk cache state without
//! touching `$XDG_CACHE_HOME`, env vars, or `#[serial]`. Cleanup is automatic
//! when the `TempDir` is dropped.
//!
//! # Toolchain gating
//!
//! Tests hard-fail with install instructions if `rust-docs-json` isn't
//! installed, **unless** `RUSTDOC_MCP_SKIP_STDLIB_TESTS=1` is set in the
//! environment. This turns silent CI skips into loud failures while still
//! giving local contributors a documented opt-out.

use assert2::check;
use rustdoc_mcp::index_metrics;
use rustdoc_mcp::stdlib::StdlibDocs;
use rustdoc_mcp::tools::inspect_item::{
    InspectItemRequest, StructuredInspectResult, handle_inspect_item_structured,
};
use rustdoc_mcp::tools::search::{SearchRequest, StructuredSearchResult, handle_search_structured};
use rustdoc_mcp::{DocState, format::DetailLevel};
use std::sync::Arc;
use tempfile::TempDir;

/// Build a `DocState` with stdlib support and an isolated cache directory.
///
/// If `rust-docs-json` isn't installed, this function respects the
/// `RUSTDOC_MCP_SKIP_STDLIB_TESTS` env var: when set, the test is skipped
/// via a `None` return; when unset, the test panics with install instructions
/// so CI failures are loud rather than silent.
fn isolated_stdlib_state() -> Option<(Arc<DocState>, TempDir)> {
    let cache_dir = TempDir::new().expect("Failed to create temp cache dir");
    let stdlib = match StdlibDocs::discover() {
        Ok(s) => s.with_cache_root(cache_dir.path().to_path_buf()),
        Err(e) => {
            if std::env::var("RUSTDOC_MCP_SKIP_STDLIB_TESTS").is_ok() {
                eprintln!("Skipping stdlib test (RUSTDOC_MCP_SKIP_STDLIB_TESTS set): {e}");
                return None;
            }
            panic!(
                "rust-docs-json component not available: {e}\n\
                 \n\
                 Install with:\n\
                 \n  rustup component add rust-docs-json --toolchain nightly\n\
                 \n\
                 Or set RUSTDOC_MCP_SKIP_STDLIB_TESTS=1 to skip this test."
            );
        }
    };
    let state = Arc::new(DocState::new(Some(Arc::new(stdlib))));
    Some((state, cache_dir))
}

/// Regression test for the primary XEV-625 failure mode: searching for
/// `HashMap` in `std` used to return "Failed to build search index for std".
///
/// Asserts on structured fields rather than substring matches: the search
/// must return `Hits`, and at least one hit must resolve to a `HashMap`
/// struct rooted under `std::collections`. The search index walks canonical
/// definition paths rather than re-exports, so the resolved path is typically
/// `std::collections::hash::map::HashMap` rather than the public
/// `std::collections::HashMap`.
#[tokio::test(flavor = "multi_thread")]
async fn search_hashmap_in_std() {
    let Some((state, _cache)) = isolated_stdlib_state() else {
        return;
    };

    let result = handle_search_structured(
        &state,
        SearchRequest {
            query: "HashMap".to_string(),
            crate_name: "std".to_string(),
            limit: Some(20),
        },
    )
    .await
    .expect("search_structured failed");

    let StructuredSearchResult::Hits {
        crate_name,
        query,
        is_stdlib,
        hits,
    } = result
    else {
        panic!("expected Hits variant, got {result:?}");
    };

    check!(crate_name == "std");
    check!(query == "HashMap");
    check!(is_stdlib);
    check!(!hits.is_empty(), "expected at least one hit");

    let hashmap_hit = hits.iter().find(|h| {
        h.full_path.starts_with("std::collections")
            && h.full_path.ends_with("::HashMap")
            && h.kind.contains("Struct")
    });
    check!(
        hashmap_hit.is_some(),
        "expected a Struct hit under std::collections::...::HashMap, got: {:?}",
        hits.iter()
            .map(|h| (&h.full_path, &h.kind))
            .collect::<Vec<_>>()
    );
}

/// Regression test for the exact query from the XEV-625 issue description:
/// `inspect_item` on `std::collections::HashMap`.
///
/// Asserts on structural fields (`full_path`, `kind`) plus a non-query
/// substring from the rendered output (`insert`, a known method name) that
/// cannot be satisfied by the query echo alone.
#[tokio::test(flavor = "multi_thread")]
async fn inspect_std_collections_hashmap() {
    let Some((state, _cache)) = isolated_stdlib_state() else {
        return;
    };

    let result = handle_inspect_item_structured(
        &state,
        InspectItemRequest {
            query: "std::collections::HashMap".to_string(),
            kind: None,
            detail_level: DetailLevel::High,
        },
    )
    .await
    .expect("inspect_item_structured failed");

    let StructuredInspectResult::Item {
        full_path,
        kind,
        crate_name,
        rendered,
    } = result
    else {
        panic!("expected Item variant, got {result:?}");
    };

    // The canonical path from rustdoc's ItemSummary points at the definition
    // site (e.g. `std::collections::hash::map::HashMap`) rather than the
    // re-export (`std::collections::HashMap`). Accept either form.
    check!(
        full_path.starts_with("std::collections") && full_path.ends_with("::HashMap"),
        "expected full_path under std::collections::...::HashMap, got {full_path:?}"
    );
    check!(
        kind.contains("Struct"),
        "expected Struct kind, got {kind:?}"
    );
    check!(crate_name == "std");
    // The rendered output should contain the struct keyword. This cannot
    // be satisfied by the query echo and confirms actual rendering ran.
    check!(
        rendered.contains("struct HashMap"),
        "expected rendered struct signature; rendered was: {rendered}"
    );
}

/// Warm-cache path: issue two searches in sequence, and use the global
/// [`index_metrics`] counter to verify the second call loaded the index
/// from disk instead of rebuilding.
///
/// Also asserts that the on-disk cache file lives under the test's isolated
/// cache directory by asking [`StdlibDocs::index_cache_path`] directly
/// (rather than walking the filesystem).
#[tokio::test(flavor = "multi_thread")]
async fn warm_cache_writes_to_isolated_dir() {
    let Some((state, _cache)) = isolated_stdlib_state() else {
        return;
    };

    let make_request = || SearchRequest {
        query: "BTreeMap".to_string(),
        crate_name: "std".to_string(),
        limit: Some(5),
    };

    let (builds_before, _) = index_metrics::snapshot();

    // Cold: builds the index and writes it to the isolated cache dir.
    let cold = handle_search_structured(&state, make_request())
        .await
        .expect("cold search failed");
    check!(matches!(cold, StructuredSearchResult::Hits { .. }));

    let (builds_after_cold, loads_after_cold) = index_metrics::snapshot();
    // At least one build must have happened between before and after_cold.
    // Parallel tests may also have built; we only assert "at least one".
    check!(
        builds_after_cold - builds_before >= 1,
        "cold call should have rebuilt the index at least once"
    );

    // Verify the expected on-disk path is populated. Query the production
    // helper so the layout stays in sync automatically if it ever changes.
    let stdlib = state.stdlib().expect("stdlib must be set on state");
    let expected_path = stdlib.index_cache_path("std");
    check!(
        expected_path.exists(),
        "expected std.index cache at {}",
        expected_path.display()
    );

    // Warm: should load from cache rather than rebuild.
    let warm = handle_search_structured(&state, make_request())
        .await
        .expect("warm search failed");
    check!(matches!(warm, StructuredSearchResult::Hits { .. }));

    let (_builds_after_warm, loads_after_warm) = index_metrics::snapshot();
    check!(
        loads_after_warm - loads_after_cold >= 1,
        "warm call should have loaded the index from disk at least once \
         (loads went from {loads_after_cold} to {loads_after_warm})"
    );
}
