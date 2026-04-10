//! TF-IDF search handler for finding documentation items.
//!
//! # Structured and rendered APIs
//!
//! This module exposes two layers:
//!
//! - [`handle_search_structured`] returns a typed [`StructuredSearchResult`]
//!   that tests and programmatic consumers can match on. It contains
//!   fully-qualified paths, kinds, relevance scores, and first doc lines —
//!   no presentation concerns.
//! - [`handle_search`] wraps the structured variant and renders it into the
//!   human-readable string format exposed over the MCP tool interface.
//!
//! Renderer changes never affect structured assertions, which eliminates the
//! brittleness of string-containment tests on MCP output.

use crate::{
    search::{QueryContext, TermIndex},
    stdlib::StdlibDocs,
    worker::DocState,
};
use rmcp::schemars;
use serde::Deserialize;
use std::{fmt::Write as _, sync::Arc};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    /// Search query term
    pub query: String,
    /// Crate to search within
    pub crate_name: String,
    /// Maximum number of results to return (default: 10)
    #[serde(default = "default_limit")]
    pub limit: Option<usize>,
}

fn default_limit() -> Option<usize> {
    Some(10)
}

/// Structured result of a search operation, independent of any string rendering.
///
/// Tests should match on this to assert on concrete fields (full paths, kinds,
/// relevance) rather than substring-matching the rendered output.
#[derive(Debug, Clone)]
pub enum StructuredSearchResult {
    /// The search ran and returned at least one hit.
    Hits {
        crate_name: String,
        query: String,
        is_stdlib: bool,
        hits: Vec<StructuredSearchHit>,
    },
    /// The search ran against a valid crate but found zero matches.
    Empty { crate_name: String, query: String },
    /// The target crate was not found. Suggestions may be provided.
    CrateNotFound {
        attempted: String,
        suggestions: Vec<CrateSuggestion>,
    },
}

/// A single search hit with all fields needed for rendering or programmatic use.
#[derive(Debug, Clone)]
pub struct StructuredSearchHit {
    /// Fully-qualified path like `std::collections::HashMap`.
    pub full_path: String,
    /// Kind as a debug-printed string (e.g. `"Struct"`, `"Function"`, `"Trait"`).
    pub kind: String,
    /// Relevance on a 0-100 scale, normalized against the top result.
    pub relevance: u32,
    /// First non-empty line of the item's doc comment, if any.
    pub first_doc_line: Option<String>,
}

/// A fuzzy crate-name suggestion surfaced when the requested crate cannot be resolved.
#[derive(Debug, Clone)]
pub struct CrateSuggestion {
    pub path: String,
    /// Optional kind (e.g. `"Crate"`, `"Module"`). When `None`, the suggestion
    /// is a bare crate name.
    pub kind: Option<String>,
}

/// Execute the search operation using TF-IDF indexing and return the rendered output.
///
/// This is the string-returning wrapper consumed by the MCP tool interface.
/// For programmatic use or tests that want to assert on structured fields,
/// call [`handle_search_structured`] directly.
#[tracing::instrument(skip_all, fields(query = %request.query, crate_name = %request.crate_name))]
pub async fn handle_search(
    state: &Arc<DocState>,
    request: SearchRequest,
) -> Result<String, String> {
    let structured = handle_search_structured(state, request).await?;
    Ok(render_search_result(&structured))
}

/// Structured variant of [`handle_search`].
///
/// Returns a [`StructuredSearchResult`] rather than a rendered string.
/// Errors (missing workspace, stdlib not available) still surface as `Err(String)`
/// so they can be displayed uniformly by callers.
#[tracing::instrument(skip_all, fields(query = %request.query, crate_name = %request.crate_name))]
pub async fn handle_search_structured(
    state: &Arc<DocState>,
    request: SearchRequest,
) -> Result<StructuredSearchResult, String> {
    // Route stdlib crates to the dedicated handler.
    if StdlibDocs::is_stdlib_crate(&request.crate_name)
        && let Some(stdlib) = state.stdlib()
    {
        tracing::debug!(crate_name = %request.crate_name, "Routing search to stdlib");
        return stdlib_search_structured(stdlib, &request).await;
    }

    // Workspace-based search.
    let workspace_ctx = match state.workspace().await {
        Some(ctx) => ctx,
        None => {
            if let Some(stdlib) = state.stdlib() {
                if StdlibDocs::is_stdlib_crate(&request.crate_name) {
                    return stdlib_search_structured(stdlib, &request).await;
                }

                return Err(format!(
                    "Crate '{}' not found. No workspace configured.\n\n\
                     Available for search without workspace:\n\
                     • Standard library: {}\n\n\
                     Use set_workspace to configure a Rust project.",
                    request.crate_name,
                    stdlib.available_crates().join(", ")
                ));
            }

            tracing::warn!("Search failed: no workspace and no stdlib available");
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

    let query_ctx = QueryContext::new(Arc::new(workspace_ctx));
    run_search(&query_ctx, &request, false)
}

/// Structured stdlib search. Shared between the direct-route and the
/// no-workspace-fallback paths in [`handle_search_structured`].
async fn stdlib_search_structured(
    stdlib: &Arc<StdlibDocs>,
    request: &SearchRequest,
) -> Result<StructuredSearchResult, String> {
    let query_ctx = stdlib.build_query_context(&request.crate_name).await?;
    run_search(&query_ctx, request, true)
}

/// Core search routine: resolves the crate, runs the query, and builds a
/// [`StructuredSearchResult`]. Shared between workspace and stdlib paths.
fn run_search(
    query_ctx: &QueryContext,
    request: &SearchRequest,
    is_stdlib: bool,
) -> Result<StructuredSearchResult, String> {
    let index = match TermIndex::load_or_build(query_ctx, &request.crate_name) {
        Ok(index) => index,
        Err(mut suggestions) => {
            tracing::debug!(
                crate_name = %request.crate_name,
                suggestions = suggestions.len(),
                "Crate not found, returning suggestions"
            );
            suggestions.sort_by(|a, b| b.score().total_cmp(&a.score()));
            let suggestions: Vec<CrateSuggestion> = suggestions
                .into_iter()
                .take(5)
                .filter(|s| s.score() > 0.8)
                .map(|s| CrateSuggestion {
                    path: s.path().to_string(),
                    kind: s.item().map(|item| format!("{:?}", item.kind())),
                })
                .collect();

            return Ok(StructuredSearchResult::CrateNotFound {
                attempted: request.crate_name.clone(),
                suggestions,
            });
        }
    };

    let limit = request.limit.unwrap_or(10);
    let matches = index.search(&request.query, limit);

    tracing::debug!(
        query = %request.query,
        crate_name = %request.crate_name,
        result_count = matches.len(),
        "Search completed"
    );

    if matches.is_empty() {
        return Ok(StructuredSearchResult::Empty {
            crate_name: request.crate_name.clone(),
            query: request.query.clone(),
        });
    }

    let max_score = matches.first().map(|r| r.rank).unwrap_or(1.0);
    let hits: Vec<StructuredSearchHit> = matches
        .iter()
        .map(|m| {
            let relevance = ((m.rank / max_score) * 100.0).round() as u32;
            match query_ctx.get_item_from_id_path(m.item.crate_name.as_str(), &m.item.item_path) {
                Some((item, path_segments)) => {
                    let full_path = path_segments.join("::");
                    let kind = format!("{:?}", item.kind());
                    let first_doc_line = item.comment().and_then(|docs| {
                        docs.lines()
                            .find(|line| !line.trim().is_empty())
                            .map(|line| line.trim().to_string())
                    });
                    StructuredSearchHit {
                        full_path,
                        kind,
                        relevance,
                        first_doc_line,
                    }
                }
                None => StructuredSearchHit {
                    full_path: "[Unable to resolve item]".to_string(),
                    kind: "Unknown".to_string(),
                    relevance,
                    first_doc_line: None,
                },
            }
        })
        .collect();

    Ok(StructuredSearchResult::Hits {
        crate_name: request.crate_name.clone(),
        query: request.query.clone(),
        is_stdlib,
        hits,
    })
}

/// Render a [`StructuredSearchResult`] into the human-readable MCP output format.
fn render_search_result(result: &StructuredSearchResult) -> String {
    match result {
        StructuredSearchResult::Hits {
            crate_name,
            query,
            is_stdlib,
            hits,
        } => render_hits(crate_name, query, *is_stdlib, hits),
        StructuredSearchResult::Empty { crate_name, query } => render_empty(crate_name, query),
        StructuredSearchResult::CrateNotFound {
            attempted,
            suggestions,
        } => render_crate_not_found(attempted, suggestions),
    }
}

fn render_hits(
    crate_name: &str,
    query: &str,
    is_stdlib: bool,
    hits: &[StructuredSearchHit],
) -> String {
    let source = if is_stdlib { " (standard library)" } else { "" };

    let mut output = format!("Search results for '{query}' in '{crate_name}'{source}:\n\n");

    for (idx, hit) in hits.iter().enumerate() {
        let _ = writeln!(
            &mut output,
            "{}. `{}` ({}) - relevance: {}%",
            idx + 1,
            hit.full_path,
            hit.kind,
            hit.relevance
        );
        if let Some(line) = &hit.first_doc_line {
            let _ = writeln!(&mut output, "   {line}");
        }
        output.push('\n');
    }

    output
}

fn render_empty(crate_name: &str, query: &str) -> String {
    let mut msg = format!("No results found for '{query}' in crate '{crate_name}'.\n\n");
    msg.push_str("Search tips:\n");
    msg.push_str("• Try a shorter or more general term\n");
    msg.push_str("• Search for types like 'HashMap', 'Vec', 'String'\n");
    msg.push_str("• Try function names like 'parse', 'read', 'write'\n");
    msg.push_str("• Search uses stemming: 'parsing' matches 'parse'\n");
    if query.contains("::") {
        msg.push_str("• Note: Search by term only, not full paths\n");
    }
    msg
}

fn render_crate_not_found(attempted: &str, suggestions: &[CrateSuggestion]) -> String {
    let mut result = format!("Crate '{attempted}' not found. Did you mean one of these?\n\n");
    for suggestion in suggestions {
        let _ = write!(&mut result, "• `{}`", suggestion.path);
        match &suggestion.kind {
            Some(kind) => {
                let _ = writeln!(&mut result, " ({kind})");
            }
            None => result.push_str(" (Crate)\n"),
        }
    }
    result
}
