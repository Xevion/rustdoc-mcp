//! Item inspection tool handler.
//!
//! # Structured and rendered APIs
//!
//! Follows the same two-layer pattern as [`crate::tools::search`]:
//!
//! - [`handle_inspect_item_structured`] returns a typed [`StructuredInspectResult`]
//!   with the resolved item's full path, kind, crate, and the rendered content
//!   (or a disambiguation list when multiple items match).
//! - [`handle_inspect_item`] wraps the structured variant and produces the
//!   human-readable MCP output.

use crate::format::DetailLevel;
use crate::format::renderers::{
    render_constant, render_enum, render_function, render_module, render_static, render_struct,
    render_trait, render_type_alias,
};
use crate::item::ItemRef;
use crate::search::{
    DetailedSearchResult, ItemKind, QueryContext, TermIndex, item_kind_str, matches_kind,
    parse_item_path, resolve_crate_from_path, score_to_percent,
};
use crate::stdlib::StdlibDocs;
use crate::types::CrateName;
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

const fn default_detail_level() -> DetailLevel {
    DetailLevel::Medium
}

/// Structured outcome of an `inspect_item` call.
///
/// Tests should match on this enum to assert on concrete fields (full path,
/// kind, candidate list) rather than substring-matching the rendered output.
#[derive(Debug, Clone)]
pub enum StructuredInspectResult {
    /// A single item was resolved. `rendered` contains the formatted output
    /// (signature + docs + detail per the requested `detail_level`).
    Item {
        full_path: String,
        kind: String,
        crate_name: String,
        rendered: String,
    },
    /// Multiple items matched; caller must disambiguate.
    Disambiguation {
        query: String,
        candidates: Vec<InspectCandidate>,
    },
}

/// One disambiguation candidate shown to the user.
#[derive(Debug, Clone)]
pub struct InspectCandidate {
    pub full_path: String,
    pub kind: String,
    pub first_doc_line: Option<String>,
}

/// Handles inspect_item requests by resolving paths or searching across crates.
///
/// This is the string-returning wrapper. For programmatic access or tests,
/// call [`handle_inspect_item_structured`] directly.
#[tracing::instrument(skip_all, fields(query = %request.query))]
pub async fn handle_inspect_item(
    state: &Arc<DocState>,
    request: InspectItemRequest,
) -> Result<String, String> {
    let structured = handle_inspect_item_structured(state, request).await?;
    Ok(render_inspect_result(&structured))
}

/// Structured variant of [`handle_inspect_item`].
///
/// Returns a typed [`StructuredInspectResult`] rather than rendered text.
/// Errors (missing workspace, no matches, kind mismatch) still surface as
/// `Err(String)` so callers can display them uniformly.
#[tracing::instrument(skip_all, fields(query = %request.query))]
#[allow(clippy::too_many_lines)]
pub async fn handle_inspect_item_structured(
    state: &Arc<DocState>,
    request: InspectItemRequest,
) -> Result<StructuredInspectResult, String> {
    // Parse the item path to check if it targets stdlib
    let path_check = parse_item_path(&request.query);

    // Check if query explicitly targets a stdlib crate
    let targets_stdlib = path_check
        .path_components
        .first()
        .is_some_and(|first| StdlibDocs::is_stdlib_crate(first));

    // If targeting stdlib and stdlib is available, handle it directly
    if targets_stdlib && let Some(stdlib) = state.stdlib() {
        tracing::debug!(query = %request.query, "Routing to stdlib handler");
        return stdlib_inspect_structured(stdlib, &request, false).await;
    }

    // Try workspace-based lookup
    let Some(workspace_ctx) = state.workspace().await else {
        // No workspace - try stdlib fallback for common types
        if let Some(stdlib) = state.stdlib() {
            return stdlib_inspect_structured(stdlib, &request, true).await;
        }

        tracing::warn!("No workspace configured and stdlib not available");
        return Err(
            "No workspace configured and standard library docs not available.\n\n\
             To configure a workspace:\n\
             • Use set_workspace with a path to a Rust project\n\n\
             To enable standard library docs:\n\
             • Run: rustup component add rust-docs-json --toolchain nightly"
                .to_string(),
        );
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
    known_crates.extend(
        workspace_ctx
            .dependency_names()
            .map(CrateName::new_unchecked),
    );

    let query_ctx = QueryContext::new(Arc::new(workspace_ctx.clone()));

    let is_path_query = path.path_components.len() > 1 || request.query.contains("::");
    let specified_crate = resolve_crate_from_path(&mut path, &known_crates);

    if is_path_query && specified_crate.is_some() {
        let crate_name = specified_crate.clone().unwrap();

        let full_path = if path.path_components.is_empty() {
            crate_name.as_str().to_string()
        } else {
            format!(
                "{}::{}",
                crate_name.as_str(),
                path.path_components.join("::")
            )
        };

        let mut suggestions = Vec::new();

        if let Some(item_ref) = query_ctx.resolve_path(&full_path, &mut suggestions) {
            tracing::debug!(path = %full_path, "Resolved item via direct path lookup");

            if let Some(kind_filter) = request.kind
                && !matches_kind(item_ref.inner(), kind_filter)
            {
                return Err(format!(
                    "Item '{}' found but is not a {:?}",
                    path.full_path(),
                    kind_filter
                ));
            }

            return build_item_result(item_ref, request.detail_level, crate_name.as_str());
        }
    }

    // Fall back to search-based resolution for non-path queries or queries without crate
    let search_query = path.full_path();

    let crates_to_search: Vec<CrateName> = if let Some(crate_name) = specified_crate {
        vec![crate_name]
    } else {
        let mut crates = workspace_ctx.members.clone();
        crates.extend(
            workspace_ctx
                .dependency_names()
                .map(CrateName::new_unchecked),
        );
        crates
    };

    let mut all_results = Vec::new();
    let mut search_failures = Vec::new();
    let mut kind_filtered_kinds: Vec<String> = Vec::new();

    const MAX_TOTAL_RESULTS: usize = 500;

    for crate_name in &crates_to_search {
        if all_results.len() >= MAX_TOTAL_RESULTS {
            tracing::debug!(
                max_results = MAX_TOTAL_RESULTS,
                "Reached maximum result limit, stopping search"
            );
            break;
        }

        let index = match TermIndex::load_or_build(&query_ctx, crate_name.as_str()) {
            Ok(index) => index,
            Err(suggestions) => {
                tracing::warn!(
                    crate_name = %crate_name,
                    suggestion_count = suggestions.len(),
                    "Failed to load search index for crate"
                );

                // Suppress "did you mean" suggestions for stdlib crates — they always
                // suggest the workspace crate, which is misleading noise.
                let error_msg = if !suggestions.is_empty()
                    && !StdlibDocs::is_stdlib_crate(crate_name.as_str())
                {
                    format!(
                        "Documentation not found (did you mean: {}?)",
                        suggestions.first().map_or("", |s| s.path.as_str())
                    )
                } else {
                    "Documentation not available".to_string()
                };
                search_failures.push((crate_name.as_str().to_string(), error_msg));
                continue;
            }
        };

        let remaining = MAX_TOTAL_RESULTS - all_results.len();
        let limit = remaining.min(50);

        let search_results = index.search(&search_query, limit);

        for search_result in search_results {
            if let Some((item_ref, path_segments)) = query_ctx.get_item_from_id_path(
                search_result.item.crate_name.as_str(),
                &search_result.item.item_path,
            ) {
                if let Some(kind_filter) = request.kind
                    && !matches_kind(item_ref.inner(), kind_filter)
                {
                    kind_filtered_kinds.push(item_kind_str(item_ref.inner()).to_string());
                    continue;
                }

                let result = DetailedSearchResult {
                    name: item_ref.name().unwrap_or("<unnamed>").to_string(),
                    path: path_segments.join("::"),
                    kind: item_kind_str(item_ref.inner()).to_string(),
                    crate_name: Some(search_result.item.crate_name.clone()),
                    docs: item_ref.comment().map(std::string::ToString::to_string),
                    id: Some(item_ref.id),
                    relevance: score_to_percent(search_result.rank),
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

    tracing::debug!(
        result_count = all_results.len(),
        crates_searched = crates_to_search.len(),
        failures = search_failures.len(),
        "Search completed"
    );

    // Deduplicate results by item ID (same item may appear at different paths due to re-exports)
    {
        let mut seen_ids = HashSet::new();
        all_results.retain(|result| {
            if let Some(id) = &result.id {
                seen_ids.insert(*id)
            } else {
                true
            }
        });
    }

    // For simple name queries (no ::), prioritize exact name matches to avoid
    // unnecessary disambiguation when user clearly wants a specific item.
    // This also guards against spurious TF-IDF hits when the query is a
    // multi-token identifier (e.g. "QueryContext" partially matching items
    // that share only the "Context" token).
    let is_simple_name = !request.query.contains("::");
    if is_simple_name && !all_results.is_empty() {
        let query_lower = request.query.to_lowercase();
        let exact_match_count = all_results
            .iter()
            .filter(|r| r.name.to_lowercase() == query_lower)
            .count();

        if exact_match_count >= 1 {
            // Prefer exact-name matches. If only one, we auto-resolve; if many,
            // disambiguation downstream still handles it correctly.
            all_results.retain(|r| r.name.to_lowercase() == query_lower);
        } else {
            // No exact matches. If the query looks like a specific identifier
            // (CamelCase or contains digits), treat partial-token hits as
            // "not found" rather than returning unrelated items.
            let looks_like_specific_name = query_lower.chars().any(|c| c.is_ascii_digit())
                || request.query.chars().any(char::is_uppercase);

            if looks_like_specific_name {
                all_results.clear();
            }
        }
    }

    if all_results.is_empty() {
        let mut error_msg = format!(
            "No items found matching '{}'{}",
            search_query,
            if let Some(k) = request.kind {
                format!(" with kind '{k:?}'")
            } else {
                String::new()
            }
        );

        if !kind_filtered_kinds.is_empty() {
            let mut unique_kinds = kind_filtered_kinds.clone();
            unique_kinds.sort();
            unique_kinds.dedup();
            let _ = write!(
                &mut error_msg,
                "\nHowever, '{}' was found as: {}",
                search_query,
                unique_kinds.join(", ")
            );
        }

        if !search_failures.is_empty() {
            error_msg.push_str("\n\nFailed to search in the following crates:");
            for (crate_name, error) in search_failures.iter().take(5) {
                let _ = write!(&mut error_msg, "\n  - {crate_name}: {error}");
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

    if !search_failures.is_empty() {
        tracing::info!(
            successful = crates_to_search.len() - search_failures.len(),
            failed = search_failures.len(),
            "Search completed with some failures"
        );
    }

    if all_results.len() > 1 {
        tracing::debug!(
            match_count = all_results.len(),
            query = %search_query,
            "Multiple matches found, returning disambiguation"
        );
        let candidates = build_candidates(
            &all_results,
            crates_to_search
                .first()
                .map(super::super::types::CrateName::as_str),
        );
        return Ok(StructuredInspectResult::Disambiguation {
            query: search_query,
            candidates,
        });
    }

    let result = &all_results[0];

    let crate_name = result
        .crate_name
        .as_ref()
        .or(result.source_crate.as_ref())
        .ok_or_else(|| "No crate information for matched item".to_string())?
        .as_str();

    let item_id = result.id.ok_or_else(|| {
        format!(
            "Item '{}' ({}) at '{}' has no ID in search results",
            result.name, result.kind, result.path
        )
    })?;

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

    if matches!(item.inner(), ItemEnum::Impl(_)) {
        return Err(format!(
            "Internal error: Found impl block for query '{}'. Please report this bug.",
            request.query
        ));
    }

    build_item_result(item, request.detail_level, crate_name)
}

/// Build a [`StructuredInspectResult::Item`] from a resolved [`ItemRef`].
///
/// Renders the item via [`format_item_output`] and captures its structural
/// fields (fully-qualified path, kind) alongside the rendered blob.
///
/// `path_string()` already includes the crate segment (e.g.
/// `"std::collections::HashMap"`), so we do **not** re-prefix with
/// `crate_name`. When the path is unavailable, fall back to
/// `{crate_name}::{name}`.
fn build_item_result(
    item: ItemRef<'_, Item>,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<StructuredInspectResult, String> {
    let kind = format!("{:?}", item.kind());
    let name = item.name().unwrap_or("<unnamed>").to_string();
    let full_path = item
        .path_string()
        .unwrap_or_else(|| format!("{crate_name}::{name}"));

    let rendered = format_item_output(item, detail_level, crate_name)?;

    Ok(StructuredInspectResult::Item {
        full_path,
        kind,
        crate_name: crate_name.to_string(),
        rendered,
    })
}

/// Convert accumulated [`DetailedSearchResult`]s into structured candidates
/// for disambiguation, preserving the crate prefix and first doc line.
fn build_candidates(
    results: &[DetailedSearchResult],
    fallback_crate: Option<&str>,
) -> Vec<InspectCandidate> {
    results
        .iter()
        .take(10)
        .map(|result| {
            let full_path = if let Some(src_crate) = &result.source_crate {
                format!("{}::{}", src_crate.as_str(), result.path)
            } else if let Some(fc) = fallback_crate {
                format!("{}::{}", fc, result.path)
            } else {
                result.path.clone()
            };

            let first_doc_line = result.docs.as_ref().and_then(|docs| {
                docs.lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
            });

            InspectCandidate {
                full_path,
                kind: result.kind.clone(),
                first_doc_line,
            }
        })
        .collect()
}

/// Render a [`StructuredInspectResult`] into the human-readable MCP output format.
fn render_inspect_result(result: &StructuredInspectResult) -> String {
    match result {
        StructuredInspectResult::Item { rendered, .. } => rendered.clone(),
        StructuredInspectResult::Disambiguation { query, candidates } => {
            format_candidates(query, candidates)
        }
    }
}

/// Render the disambiguation candidate list for a failing single-item resolution.
fn format_candidates(query: &str, candidates: &[InspectCandidate]) -> String {
    let mut error =
        format!("\n// Multiple items found matching '{query}'. Please be more specific:\n\n");

    for (i, cand) in candidates.iter().enumerate() {
        let _ = write!(&mut error, "{}. {} [{}]", i + 1, cand.full_path, cand.kind);
        if let Some(line) = &cand.first_doc_line
            && !line.is_empty()
        {
            let _ = write!(&mut error, " - {line}");
        }
        let _ = writeln!(&mut error);
    }

    error
}

/// Format item output based on type and verbosity.
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
        ItemEnum::Macro(_) | ItemEnum::ProcMacro(_) => {
            return Err(format!(
                "'{}' is a macro; macros are not currently supported by inspect_item",
                item.name().unwrap_or("unknown")
            ));
        }
        _ => {
            return Err(format!(
                "Unsupported item type: {}",
                crate::search::item_kind_str(item.inner())
            ));
        }
    };

    result.map_err(|e| format!("Formatting error: {e}"))?;
    Ok(output)
}

/// Structured handler for stdlib `inspect_item` queries.
///
/// If `add_no_workspace_hint` is true, the rendered Item output will be
/// postfixed with a hint that no workspace is configured — preserving the
/// behavior of the previous string handler for the workspace-less fallback.
async fn stdlib_inspect_structured(
    stdlib: &Arc<StdlibDocs>,
    request: &InspectItemRequest,
    add_no_workspace_hint: bool,
) -> Result<StructuredInspectResult, String> {
    let path = parse_item_path(&request.query);

    let (target_crate, search_name) = if let Some(first) = path.path_components.first() {
        if StdlibDocs::is_stdlib_crate(first) {
            let remaining: Vec<_> = path.path_components.iter().skip(1).cloned().collect();
            let search = if remaining.is_empty() {
                first.clone()
            } else {
                remaining.last().cloned().unwrap_or_default()
            };
            (first.clone(), search)
        } else {
            ("std".to_string(), first.clone())
        }
    } else {
        return Err("Empty query".to_string());
    };

    let query_ctx = stdlib.build_query_context(&target_crate).await?;

    let full_path = if path.path_components.len() > 1 {
        path.path_components.join("::")
    } else {
        format!("{target_crate}::{search_name}")
    };

    // Path resolution first
    let path_result: Option<Result<StructuredInspectResult, String>> = {
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
                build_item_result(item_ref, request.detail_level, &target_crate)
            })
    };

    if let Some(result) = path_result {
        return maybe_append_hint(result, add_no_workspace_hint);
    }

    // Fall back to search
    let index = TermIndex::load_or_build(&query_ctx, &target_crate)
        .map_err(|_| format!("Failed to build search index for {target_crate}"))?;

    let results = index.search(&search_name, 10);
    let available_crates = stdlib.available_crates();

    if results.is_empty() {
        tracing::debug!(
            query = %request.query,
            crate_name = %target_crate,
            "Item not found in stdlib"
        );
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

    let best = &results[0];
    if let Some((item_ref, _)) =
        query_ctx.get_item_from_id_path(best.item.crate_name.as_str(), &best.item.item_path)
    {
        if let Some(kind_filter) = request.kind
            && !matches_kind(item_ref.inner(), kind_filter)
        {
            return Err(format!(
                "Item '{}' found but is not a {:?}",
                request.query, kind_filter
            ));
        }
        let result = build_item_result(item_ref, request.detail_level, &target_crate);
        return maybe_append_hint(result, add_no_workspace_hint);
    }

    tracing::debug!(query = %request.query, "Failed to resolve item in stdlib");
    Err(format!("Failed to resolve item '{}'", request.query))
}

/// Append a "no workspace configured" hint to a stdlib Item result's rendered
/// output, matching the behavior of the original string handler's fallback.
fn maybe_append_hint(
    result: Result<StructuredInspectResult, String>,
    add_hint: bool,
) -> Result<StructuredInspectResult, String> {
    if !add_hint {
        return result;
    }
    result.map(|r| match r {
        StructuredInspectResult::Item {
            full_path,
            kind,
            crate_name,
            mut rendered,
        } => {
            rendered.push_str(
                "\n---\nNote: No workspace configured. Showing standard library only.\n\
                 Use set_workspace to search additional crates.",
            );
            StructuredInspectResult::Item {
                full_path,
                kind,
                crate_name,
                rendered,
            }
        }
        other @ StructuredInspectResult::Disambiguation { .. } => other,
    })
}
