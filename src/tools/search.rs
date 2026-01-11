//! TF-IDF search handler for finding documentation items.

use crate::{
    search::{QueryContext, TermIndex},
    stdlib::StdlibDocs,
    worker::DocState,
    workspace::{CrateMetadata, CrateOrigin, WorkspaceContext},
};
use rmcp::schemars;
use serde::Deserialize;
use std::{collections::HashMap, fmt::Write as _, path::PathBuf, sync::Arc};

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

/// Execute the search operation using TF-IDF indexing.
pub async fn handle_search(
    state: &Arc<DocState>,
    request: SearchRequest,
) -> Result<String, String> {
    // Check if searching a stdlib crate
    if StdlibDocs::is_stdlib_crate(&request.crate_name)
        && let Some(stdlib) = state.stdlib()
    {
        return handle_stdlib_search(stdlib, &request).await;
    }

    // Try workspace-based search
    let workspace_ctx = match state.workspace().await {
        Some(ctx) => ctx,
        None => {
            // No workspace - check if we can search stdlib
            if let Some(stdlib) = state.stdlib() {
                if StdlibDocs::is_stdlib_crate(&request.crate_name) {
                    return handle_stdlib_search(stdlib, &request).await;
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

    // Create QueryContext for this operation
    let query_ctx = QueryContext::new(Arc::new(workspace_ctx.clone()));

    // Load or build search index
    let index = match TermIndex::load_or_build(&query_ctx, &request.crate_name) {
        Ok(index) => index,
        Err(mut suggestions) => {
            // Format suggestions for crate name
            let mut result = format!(
                "Crate '{}' not found. Did you mean one of these?\n\n",
                &request.crate_name
            );
            suggestions.sort_by(|a, b| b.score().total_cmp(&a.score()));
            for suggestion in suggestions.into_iter().take(5).filter(|s| s.score() > 0.8) {
                result
                    .write_fmt(format_args!("• `{}`", suggestion.path()))
                    .unwrap();

                if let Some(item) = suggestion.item() {
                    result
                        .write_fmt(format_args!(" ({:?})\n", item.kind()))
                        .unwrap();
                } else {
                    result.push_str(" (Crate)\n");
                }
            }
            return Ok(result);
        }
    };

    // Perform search
    let limit = request.limit.unwrap_or(10);
    let results = index.search(&request.query, limit);

    if results.is_empty() {
        let mut msg = format!(
            "No results found for '{}' in crate '{}'.\n\n",
            request.query, request.crate_name
        );

        // Provide helpful suggestions
        msg.push_str("Search tips:\n");
        msg.push_str("• Try a shorter or more general term\n");
        msg.push_str("• Search for types like 'HashMap', 'Vec', 'String'\n");
        msg.push_str("• Try function names like 'parse', 'read', 'write'\n");
        msg.push_str("• Search uses stemming: 'parsing' matches 'parse'\n");

        // Check if query looks like it might be too specific
        if request.query.contains("::") {
            msg.push_str("• Note: Search by term only, not full paths\n");
        }

        return Ok(msg);
    }

    Ok(format_search_results(
        &results,
        &request.query,
        &request.crate_name,
        &query_ctx,
        false,
    ))
}

/// Format search results into a readable string output.
fn format_search_results(
    results: &[crate::search::SearchMatch],
    query: &str,
    crate_name: &str,
    query_ctx: &QueryContext,
    is_stdlib: bool,
) -> String {
    let source = if is_stdlib { " (standard library)" } else { "" };

    let mut output = format!(
        "Search results for '{}' in '{}'{}:\n\n",
        query, crate_name, source
    );

    let max_score = results.first().map(|r| r.rank).unwrap_or(1.0);

    for (idx, result) in results.iter().enumerate() {
        let relevance = ((result.rank / max_score) * 100.0).round() as u8;

        match query_ctx.get_item_from_id_path(&result.item.crate_name, &result.item.item_path) {
            Some((item, path_segments)) => {
                let path = path_segments.join("::");
                output
                    .write_fmt(format_args!(
                        "{}. `{}` ({:?}) - relevance: {}%\n",
                        idx + 1,
                        path,
                        item.kind(),
                        relevance
                    ))
                    .unwrap();

                if let Some(docs) = item.comment() {
                    let first_line = docs
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("");
                    if !first_line.is_empty() {
                        output
                            .write_fmt(format_args!("   {}\n", first_line.trim()))
                            .unwrap();
                    }
                }
            }
            None => {
                output
                    .write_fmt(format_args!(
                        "{}. [Unable to resolve item] - relevance: {}%\n",
                        idx + 1,
                        relevance
                    ))
                    .unwrap();
            }
        };

        output.push('\n');
    }

    output
}

/// Handle search for stdlib crates without a workspace.
async fn handle_stdlib_search(
    stdlib: &Arc<StdlibDocs>,
    request: &SearchRequest,
) -> Result<String, String> {
    // Load the stdlib crate docs
    let _crate_index = stdlib
        .load(&request.crate_name)
        .await
        .map_err(|e| format!("Failed to load {} documentation: {}", request.crate_name, e))?;

    // Build a minimal workspace context for stdlib
    let mut crate_info = HashMap::new();
    crate_info.insert(
        request.crate_name.clone(),
        CrateMetadata {
            origin: CrateOrigin::Standard,
            name: request.crate_name.clone(),
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

    // Load or build search index
    let index = TermIndex::load_or_build(&query_ctx, &request.crate_name)
        .map_err(|_| format!("Failed to build search index for {}", request.crate_name))?;

    // Perform search
    let limit = request.limit.unwrap_or(10);
    let results = index.search(&request.query, limit);

    if results.is_empty() {
        let mut msg = format!(
            "No results found for '{}' in '{}'.\n\n",
            request.query, request.crate_name
        );

        msg.push_str("Search tips:\n");
        msg.push_str("• Try a shorter or more general term\n");
        msg.push_str("• Search for types like 'HashMap', 'Vec', 'String'\n");
        msg.push_str("• Try function names like 'parse', 'read', 'write'\n");

        return Ok(msg);
    }

    Ok(format_search_results(
        &results,
        &request.query,
        &request.crate_name,
        &query_ctx,
        true,
    ))
}
