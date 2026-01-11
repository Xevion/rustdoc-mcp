use crate::format::DetailLevel;
use crate::format::renderers::*;
use crate::item::ItemRef;
use crate::search::{
    DetailedSearchResult, ItemKind, QueryContext, TermIndex, item_kind_str, matches_kind,
    parse_item_path, resolve_crate_from_path,
};
use crate::stdlib::StdlibDocs;
use crate::worker::DocState;

use rmcp::schemars;
use rustdoc_types::{Item, ItemEnum};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InspectItemRequest {
    /// Item to inspect (e.g., "Vec", "std::vec::Vec", "HashMap")
    pub query: String,
    /// Optional filter by item kind (struct, enum, function, trait, module, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ItemKind>,
    /// Detail level: low (signature only), medium (+docs), high (+members+impls)
    #[serde(default = "default_detail_level")]
    pub detail_level: DetailLevel,
}

fn default_detail_level() -> DetailLevel {
    DetailLevel::Medium
}

/// Handles inspect_item requests by resolving paths or searching across crates.
/// Attempts path resolution first for explicit paths, falls back to fuzzy search if needed.
pub async fn handle_inspect_item(
    state: &Arc<DocState>,
    request: InspectItemRequest,
) -> Result<String, String> {
    // Parse the item path to check if it targets stdlib
    let path_check = parse_item_path(&request.query);

    // Check if query explicitly targets a stdlib crate
    let targets_stdlib = path_check
        .path_components
        .first()
        .map(|first| StdlibDocs::is_stdlib_crate(first))
        .unwrap_or(false);

    // If targeting stdlib and stdlib is available, handle it directly
    if targets_stdlib && let Some(stdlib) = state.stdlib() {
        return handle_stdlib_inspect(stdlib, &request).await;
    }

    // Try workspace-based lookup
    let workspace_ctx = match state.workspace().await {
        Some(ctx) => ctx,
        None => {
            // No workspace - try stdlib fallback for common types
            if let Some(stdlib) = state.stdlib() {
                // Try to find common types in stdlib (Vec, HashMap, String, etc.)
                return handle_stdlib_inspect(stdlib, &request).await
                    .map(|mut result| {
                        // Add a hint that we're showing stdlib only
                        let hint = "\n---\nNote: No workspace configured. Showing standard library only.\n\
                                    Use set_workspace to search additional crates.";
                        result.push_str(hint);
                        result
                    });
            }

            return Err(
                "No workspace configured and standard library docs not available.\n\n\
                 To configure a workspace:\n\
                 • Use set_workspace with a path to a Rust project\n\n\
                 To enable standard library docs:\n\
                 • Run: rustup component add rust-docs-json --toolchain nightly"
                    .to_string(),
            );
        }
    };

    // Parse the item path
    let mut path = parse_item_path(&request.query);

    // Verify workspace is configured
    state
        .working_directory()
        .await
        .ok_or_else(|| "No working directory configured. Use set_workspace first.".to_string())?;

    // Build list of known crates (members + dependencies)
    let mut known_crates = workspace_ctx.members.clone();
    known_crates.extend(workspace_ctx.dependency_names().map(|s| s.to_string()));

    // Create single QueryContext for the entire request (path resolution, search, and module traversal)
    let query_ctx = QueryContext::new(Arc::new(workspace_ctx.clone()));

    // Check if this is a path query (contains ::)
    let is_path_query = path.path_components.len() > 1 || request.query.contains("::");

    // Check if the query specifies a crate (e.g., "serde::Serialize")
    let specified_crate = resolve_crate_from_path(&mut path, &known_crates);

    if is_path_query && specified_crate.is_some() {
        // Path query with explicit crate - try resolve_path first
        let crate_name = specified_crate.clone().unwrap();

        // Build full path string (crate_name::path::components)
        let full_path = if path.path_components.is_empty() {
            crate_name.clone()
        } else {
            format!("{}::{}", crate_name, path.path_components.join("::"))
        };

        let mut suggestions = Vec::new();

        // Try path resolution for direct lookup
        if let Some(item_ref) = query_ctx.resolve_path(&full_path, &mut suggestions) {
            // Apply kind filter if specified
            if let Some(kind_filter) = request.kind
                && !matches_kind(item_ref.inner(), kind_filter)
            {
                return Err(format!(
                    "Item '{}' found but is not a {:?}",
                    path.full_path(),
                    kind_filter
                ));
            }

            // Use ItemRef directly - no need to reload documentation
            return format_item_output(item_ref, request.detail_level, &crate_name);
        }

        // Path resolution failed - fall back to search within this crate
        // (handles re-exports that aren't in the module hierarchy)
    }

    // Fall back to search-based resolution for non-path queries or queries without crate
    let search_query = path.full_path();

    // Determine which crates to search
    let crates_to_search: Vec<String> = if let Some(crate_name) = specified_crate {
        vec![crate_name]
    } else {
        let mut crates = workspace_ctx.members.clone();
        crates.extend(workspace_ctx.dependency_names().map(|s| s.to_string()));
        crates
    };

    // Search across all target crates using TF-IDF
    let mut all_results = Vec::new();
    let mut search_failures = Vec::new();

    // Limit total results to prevent unbounded memory growth
    const MAX_TOTAL_RESULTS: usize = 500;

    // Reuse the existing QueryContext for search operations
    for crate_name in &crates_to_search {
        // Early termination if we have enough results
        if all_results.len() >= MAX_TOTAL_RESULTS {
            tracing::debug!(
                "Reached maximum result limit ({}), stopping search",
                MAX_TOTAL_RESULTS
            );
            break;
        }

        // Load search index for this crate
        let index = match TermIndex::load_or_build(&query_ctx, crate_name) {
            Ok(index) => index,
            Err(suggestions) => {
                // Log the failure for debugging
                tracing::warn!(
                    crate_name = %crate_name,
                    suggestion_count = suggestions.len(),
                    "Failed to load search index for crate"
                );

                // Track for user-facing error messages
                let error_msg = if suggestions.is_empty() {
                    "Documentation not found or failed to load".to_string()
                } else {
                    format!(
                        "Documentation not found (did you mean: {}?)",
                        suggestions.first().map(|s| s.path.as_str()).unwrap_or("")
                    )
                };
                search_failures.push((crate_name.clone(), error_msg));
                continue;
            }
        };

        // Calculate how many results we can still accept
        let remaining = MAX_TOTAL_RESULTS - all_results.len();
        let limit = remaining.min(50);

        // Perform TF-IDF search
        let search_results = index.search(&search_query, limit);

        // Convert indexer::SearchResult to types::DetailedSearchResult and filter
        for search_result in search_results {
            // Resolve the item from item_path
            if let Some((item_ref, path_segments)) = query_ctx.get_item_from_id_path(
                &search_result.item.crate_name,
                &search_result.item.item_path,
            ) {
                // Apply kind filter if specified
                if let Some(kind_filter) = request.kind
                    && !matches_kind(item_ref.inner(), kind_filter)
                {
                    continue;
                }

                // Convert to old SearchResult format for compatibility
                let result = DetailedSearchResult {
                    name: item_ref.name().unwrap_or("<unnamed>").to_string(),
                    path: path_segments.join("::"),
                    kind: item_kind_str(item_ref.inner()).to_string(),
                    crate_name: Some(search_result.item.crate_name.clone()),
                    docs: item_ref.comment().map(|s| s.to_string()),
                    id: Some(item_ref.id),
                    relevance: (search_result.rank * 100.0) as u32, // Convert float rank to u32
                    source_crate: Some(crate_name.clone()),
                };

                all_results.push(result);
            }
        }
    }

    all_results.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then_with(|| a.name.cmp(&b.name))
    });

    // Deduplicate results by item ID (same item may appear at different paths due to re-exports)
    {
        let mut seen_ids = HashSet::new();
        all_results.retain(|result| {
            if let Some(id) = &result.id {
                seen_ids.insert(*id)
            } else {
                true // Keep items without IDs
            }
        });
    }

    // For simple name queries (no ::), prioritize exact name matches to avoid
    // unnecessary disambiguation when user clearly wants a specific item
    let is_simple_name = !request.query.contains("::");
    if is_simple_name && all_results.len() > 1 {
        let query_lower = request.query.to_lowercase();
        let exact_match_count = all_results
            .iter()
            .filter(|r| r.name.to_lowercase() == query_lower)
            .count();

        // If exactly one item has an exact name match, filter to just that item
        if exact_match_count == 1 {
            all_results.retain(|r| r.name.to_lowercase() == query_lower);
        } else if exact_match_count == 0 {
            // No exact matches - check if the query looks like a specific identifier
            // (CamelCase or contains numbers) that should match exactly
            let looks_like_specific_name = query_lower.chars().any(|c| c.is_ascii_digit())
                || request.query.chars().any(|c| c.is_uppercase());

            if looks_like_specific_name {
                // Query looks like a specific identifier but no exact match exists
                // Treat as "not found" rather than showing unrelated partial matches
                all_results.clear();
            }
        }
        // If 2+ exact matches, fall through to normal disambiguation
    }

    if all_results.is_empty() {
        let mut error_msg = format!(
            "No items found matching '{}'{}",
            search_query,
            if let Some(k) = request.kind {
                format!(" with kind '{:?}'", k)
            } else {
                String::new()
            }
        );

        // Add failure context if crates failed to load
        if !search_failures.is_empty() {
            error_msg.push_str("\n\nFailed to search in the following crates:");
            for (crate_name, error) in search_failures.iter().take(5) {
                let _ = write!(&mut error_msg, "\n  - {}: {}", crate_name, error);
            }

            if search_failures.len() > 5 {
                let _ = write!(
                    &mut error_msg,
                    "\n  ... and {} more",
                    search_failures.len() - 5
                );
            }
        }

        return Err(error_msg);
    }

    // Log if we have results but also had some failures
    if !search_failures.is_empty() {
        tracing::info!(
            successful = crates_to_search.len() - search_failures.len(),
            failed = search_failures.len(),
            "Search completed with some failures"
        );
    }

    if all_results.len() > 1 {
        return Err(format_disambiguation_error(
            &all_results,
            &search_query,
            crates_to_search.first().unwrap(),
        ));
    }

    let result = &all_results[0];

    let crate_name = result
        .crate_name
        .as_ref()
        .or(result.source_crate.as_ref())
        .ok_or_else(|| "No crate information for matched item".to_string())?;

    // Get the item directly from the already-loaded documentation via QueryContext
    // This avoids reloading docs which would fail in isolated test environments
    let item_id = result.id.as_ref().ok_or_else(|| {
        format!(
            "Item '{}' ({}) at '{}' has no ID in search results",
            result.name, result.kind, result.path
        )
    })?;

    // Try to get the item from the query context's cache (already loaded during search)
    let item = query_ctx
        .load_crate(crate_name)
        .ok()
        .and_then(|crate_index| crate_index.get(&query_ctx, item_id))
        .ok_or_else(|| {
            format!(
                "Item '{}' ({}) found at '{}' but documentation not loaded",
                result.name, result.kind, result.path
            )
        })?;

    // Skip impl blocks (shouldn't happen, but safeguard)
    if matches!(item.inner(), ItemEnum::Impl(_)) {
        return Err(format!(
            "Internal error: Found impl block for query '{}'. Please report this bug.",
            request.query
        ));
    }

    format_item_output(item, request.detail_level, crate_name)
}

/// Format a disambiguation error when multiple items match
fn format_disambiguation_error(
    results: &[DetailedSearchResult],
    query: &str,
    crate_name: &str,
) -> String {
    let mut error = format!(
        "Multiple items found matching '{}'. Please be more specific:\n\n",
        query
    );

    for (i, result) in results.iter().enumerate().take(10) {
        // Show crate name prefix in the path
        let full_path = if let Some(src_crate) = &result.source_crate {
            format!("{}::{}", src_crate, result.path)
        } else {
            format!("{}::{}", crate_name, result.path)
        };

        let _ = write!(&mut error, "{}. {} [{}]", i + 1, full_path, result.kind);

        // Only show docs if they exist and are non-empty
        if let Some(docs) = &result.docs {
            let docs_trimmed = docs.trim();
            if !docs_trimmed.is_empty()
                && let Some(first_line) = docs_trimmed.lines().next()
            {
                let first_line_trimmed = first_line.trim();
                if !first_line_trimmed.is_empty() {
                    let _ = write!(&mut error, " - {}", first_line_trimmed);
                }
            }
        }

        let _ = writeln!(&mut error);
    }

    if results.len() > 10 {
        let _ = writeln!(&mut error, "\n... and {} more matches", results.len() - 10);
    }

    error
}

/// Format item output based on type and verbosity
fn format_item_output(
    item: ItemRef<'_, Item>,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<String, String> {
    let mut output = String::new();

    let result = match item.inner() {
        ItemEnum::Struct(s) => render_struct(&mut output, item, s, detail_level, crate_name),
        ItemEnum::Enum(e) => render_enum(&mut output, item, e, detail_level, crate_name),
        ItemEnum::Function(f) => render_function(&mut output, item, f, detail_level, crate_name),
        ItemEnum::Trait(t) => render_trait(&mut output, item, t, detail_level, crate_name),
        ItemEnum::Module(_) => render_module(&mut output, item, detail_level, crate_name),
        ItemEnum::TypeAlias(ta) => {
            render_type_alias(&mut output, item, ta, detail_level, crate_name)
        }
        ItemEnum::Constant { type_, const_: _ } => {
            render_constant(&mut output, item, type_, detail_level, crate_name)
        }
        ItemEnum::Static(s) => render_static(&mut output, item, s, detail_level, crate_name),
        _ => Err(format!("Unsupported item type: {:?}", item.inner())),
    };

    result?;
    Ok(output)
}

/// Handle inspect_item for stdlib crates when no workspace is available.
async fn handle_stdlib_inspect(
    stdlib: &Arc<crate::stdlib::StdlibDocs>,
    request: &InspectItemRequest,
) -> Result<String, String> {
    let path = parse_item_path(&request.query);

    // Determine which stdlib crate to search
    let (target_crate, search_name) = if let Some(first) = path.path_components.first() {
        if StdlibDocs::is_stdlib_crate(first) {
            // Query is like "std::vec::Vec" - search in specified crate
            let remaining: Vec<_> = path.path_components.iter().skip(1).cloned().collect();
            let search = if remaining.is_empty() {
                first.clone() // Just "std" - return the crate root
            } else {
                remaining.last().cloned().unwrap_or_default()
            };
            (first.clone(), search)
        } else {
            // Query is like "Vec" - search in std first
            ("std".to_string(), first.clone())
        }
    } else {
        return Err("Empty query".to_string());
    };

    // Load the stdlib crate
    let _crate_index = stdlib
        .load(&target_crate)
        .await
        .map_err(|e| format!("Failed to load {} documentation: {}", target_crate, e))?;

    // Build a minimal workspace context for stdlib
    use crate::workspace::{CrateMetadata, CrateOrigin, WorkspaceContext};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let mut crate_info = HashMap::new();
    crate_info.insert(
        target_crate.clone(),
        CrateMetadata {
            origin: CrateOrigin::Standard,
            name: target_crate.clone(),
            version: Some("nightly".to_string()),
            description: None,
            dev_dep: false,
            is_root_crate: false,
            used_by: vec![],
        },
    );

    let stdlib_ctx = WorkspaceContext {
        root: PathBuf::from("/"),
        members: vec![],
        crate_info,
        root_crate: None,
    };

    let query_ctx = QueryContext::new(Arc::new(stdlib_ctx));

    // Try path resolution first
    let full_path = if path.path_components.len() > 1 {
        path.path_components.join("::")
    } else {
        format!("{}::{}", target_crate, search_name)
    };

    // Path resolution - use a scope to ensure suggestions is dropped before any await
    let path_result = {
        let mut suggestions = Vec::new();
        query_ctx
            .resolve_path(&full_path, &mut suggestions)
            .map(|item_ref| {
                if let Some(kind_filter) = request.kind
                    && !matches_kind(item_ref.inner(), kind_filter)
                {
                    return Err(format!(
                        "Item '{}' found but is not a {:?}",
                        request.query, kind_filter
                    ));
                }
                format_item_output(item_ref, request.detail_level, &target_crate)
            })
    };

    if let Some(result) = path_result {
        return result;
    }

    // Fall back to search
    let index = TermIndex::load_or_build(&query_ctx, &target_crate)
        .map_err(|_| format!("Failed to build search index for {}", target_crate))?;

    let results = index.search(&search_name, 10);
    let available_crates = stdlib.available_crates();

    if results.is_empty() {
        return Err(format!(
            "Item '{}' not found in {}.\n\n\
             Try:\n\
             • Searching in a different stdlib crate: {}\n\
             • Using a more specific path like 'std::collections::HashMap'",
            request.query,
            target_crate,
            available_crates.join(", ")
        ));
    }

    // Get the best match
    let best = &results[0];
    if let Some((item_ref, _path_segments)) =
        query_ctx.get_item_from_id_path(&best.item.crate_name, &best.item.item_path)
    {
        if let Some(kind_filter) = request.kind
            && !matches_kind(item_ref.inner(), kind_filter)
        {
            return Err(format!(
                "Item '{}' found but is not a {:?}",
                request.query, kind_filter
            ));
        }
        return format_item_output(item_ref, request.detail_level, &target_crate);
    }

    Err(format!("Failed to resolve item '{}'", request.query))
}
