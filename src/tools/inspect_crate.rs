use crate::error::Result;
use crate::format::DetailLevel;
use crate::server::ServerContext;
use crate::workspace::{CrateOrigin, get_docs};
use anyhow::anyhow;
use rmcp::schemars;
use rustdoc_types::ItemEnum;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InspectCrateRequest {
    /// Crate to inspect. If omitted, shows summary of all crates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crate_name: Option<String>,

    /// Detail level: low (counts only), medium (+ modules), high (+ top exports)
    #[serde(default = "default_detail_level")]
    pub detail_level: DetailLevel,
}

fn default_detail_level() -> DetailLevel {
    DetailLevel::Medium
}

/// Handles inspect_crate requests by showing crate-level information.
///
/// ## Summary Mode (no crate_name)
/// Lists all available crates with:
/// - Name, version, origin
/// - Description (if available)
/// - Basic statistics
///
/// ## Detail Mode (with crate_name)
/// Shows detailed information about a specific crate:
/// - Module hierarchy
/// - Item counts by kind
/// - Common exports
/// - Usage information
pub async fn handle_inspect_crate(
    context: &ServerContext,
    request: InspectCrateRequest,
) -> Result<String> {
    let workspace_ctx = context
        .workspace_context()
        .ok_or_else(|| anyhow!("Workspace not configured. Use set_workspace tool first."))?;

    match request.crate_name {
        None => render_summary_mode(workspace_ctx, request.detail_level, context).await,
        Some(crate_name) => {
            render_detail_mode(&crate_name, workspace_ctx, request.detail_level, context).await
        }
    }
}

/// Summary mode: list all crates with descriptions and stats
async fn render_summary_mode(
    workspace_ctx: &crate::workspace::WorkspaceContext,
    detail_level: DetailLevel,
    _context: &ServerContext,
) -> Result<String> {
    let mut output = String::new();

    // Categorize crates
    let mut workspace_members = Vec::new();
    let mut external_deps = Vec::new();
    let mut std_crates = Vec::new();

    for (name, metadata) in &workspace_ctx.crate_info {
        match metadata.origin {
            CrateOrigin::Local => workspace_members.push((name, metadata)),
            CrateOrigin::External => external_deps.push((name, metadata)),
            CrateOrigin::Standard => std_crates.push((name, metadata)),
        }
    }

    // Sort by usage (most used first), then alphabetically
    external_deps.sort_by(|(name_a, meta_a), (name_b, meta_b)| {
        meta_b
            .used_by
            .len()
            .cmp(&meta_a.used_by.len())
            .then_with(|| name_a.cmp(name_b))
    });

    // Workspace Members
    if !workspace_members.is_empty() {
        writeln!(output, "Workspace Members ({}):", workspace_members.len())?;
        for (name, meta) in &workspace_members {
            let version = meta.version.as_deref().unwrap_or("unknown");
            write!(output, "  • {} v{}", name, version)?;
            if meta.is_root_crate {
                write!(output, " (root)")?;
            }
            writeln!(output)?;

            if detail_level != DetailLevel::Low
                && let Some(desc) = &meta.description
            {
                writeln!(output, "    {}", truncate_description(desc, 80))?;
            }
        }
        writeln!(output)?;
    }

    // External Dependencies
    if !external_deps.is_empty() {
        writeln!(output, "External Dependencies ({}):", external_deps.len())?;

        let limit = match detail_level {
            DetailLevel::Low => 10,
            DetailLevel::Medium => 20,
            DetailLevel::High => external_deps.len(),
        };

        for (name, meta) in external_deps.iter().take(limit) {
            let version = meta.version.as_deref().unwrap_or("unknown");
            write!(output, "  • {} v{}", name, version)?;

            if detail_level != DetailLevel::Low && !meta.used_by.is_empty() {
                write!(output, " (used by {})", meta.used_by.join(", "))?;
            }
            writeln!(output)?;

            if detail_level == DetailLevel::High
                && let Some(desc) = &meta.description
            {
                writeln!(output, "    {}", truncate_description(desc, 80))?;
            }
        }

        if external_deps.len() > limit {
            writeln!(
                output,
                "  ... and {} more dependencies",
                external_deps.len() - limit
            )?;
        }
        writeln!(output)?;
    }

    // Standard Library
    if !std_crates.is_empty() && detail_level != DetailLevel::Low {
        writeln!(output, "Standard Library ({}):", std_crates.len())?;
        std_crates.sort_by_key(|(name, _)| *name);
        for (name, _) in std_crates.iter().take(5) {
            writeln!(output, "  • {}", name)?;
        }
        if std_crates.len() > 5 {
            writeln!(output, "  ... and {} more", std_crates.len() - 5)?;
        }
    }

    Ok(output)
}

/// Detail mode: deep dive into a specific crate
async fn render_detail_mode(
    crate_name: &str,
    workspace_ctx: &crate::workspace::WorkspaceContext,
    detail_level: DetailLevel,
    context: &ServerContext,
) -> Result<String> {
    let mut output = String::new();

    // Get crate metadata
    let meta = workspace_ctx
        .get_crate(crate_name)
        .ok_or_else(|| anyhow!("Crate '{}' not found in workspace", crate_name))?;

    // Header
    let version = meta.version.as_deref().unwrap_or("unknown");
    writeln!(output, "Crate: {} v{}", crate_name, version)?;
    writeln!(output, "Origin: {:?}", meta.origin)?;

    if let Some(desc) = &meta.description {
        writeln!(output, "\n{}", desc)?;
    }

    // Usage information
    if !meta.used_by.is_empty() {
        writeln!(output, "\nUsed by: {}", meta.used_by.join(", "))?;
    }

    // Try to load documentation
    let workspace_root = context
        .working_directory()
        .ok_or_else(|| anyhow!("No working directory configured"))?;

    let cargo_lock_path = context.cargo_lock_path().map(|p| p.as_path());

    let is_workspace_member = meta.origin == CrateOrigin::Local;
    let version = meta.version.as_deref();

    let doc_result = get_docs(
        crate_name,
        version,
        workspace_root,
        is_workspace_member,
        cargo_lock_path,
    )
    .await;

    match doc_result {
        Ok(crate_index) => {
            writeln!(output, "\nDocumentation: Available")?;

            // Item counts
            let counts = count_items_by_kind(&crate_index);
            writeln!(output, "\nItem Counts:")?;
            for (kind, count) in &counts {
                writeln!(output, "  {}: {}", kind, count)?;
            }

            // Module hierarchy (medium and high detail)
            if detail_level != DetailLevel::Low
                && let Some(root) = crate_index.root_module()
                && let ItemEnum::Module(module) = &root.inner
            {
                writeln!(output, "\nTop-level Modules:")?;
                let mut module_names: Vec<_> = module
                    .items
                    .iter()
                    .filter_map(|id| {
                        let item = crate_index.get_item(id)?;
                        if matches!(item.inner, ItemEnum::Module(_)) {
                            item.name.as_ref()
                        } else {
                            None
                        }
                    })
                    .collect();
                module_names.sort();

                let limit = if detail_level == DetailLevel::High {
                    module_names.len()
                } else {
                    10
                };

                for name in module_names.iter().take(limit) {
                    writeln!(output, "  • {}", name)?;
                }

                if module_names.len() > limit {
                    writeln!(
                        output,
                        "  ... and {} more modules",
                        module_names.len() - limit
                    )?;
                }
            }

            // Top exports (high detail only)
            if detail_level == DetailLevel::High {
                writeln!(output, "\nCommon Exports:")?;

                // Show top types
                let types = crate_index.public_types();
                if !types.is_empty() {
                    writeln!(output, "  Types:")?;
                    for item in types.iter().take(5) {
                        if item.name.is_some() {
                            let path = crate_index.get_item_path(item);
                            writeln!(output, "    • {}", path)?;
                        }
                    }
                    if types.len() > 5 {
                        writeln!(output, "    ... and {} more types", types.len() - 5)?;
                    }
                }

                // Show top traits
                let traits = crate_index.public_traits();
                if !traits.is_empty() {
                    writeln!(output, "  Traits:")?;
                    for item in traits.iter().take(5) {
                        if item.name.is_some() {
                            let path = crate_index.get_item_path(item);
                            writeln!(output, "    • {}", path)?;
                        }
                    }
                    if traits.len() > 5 {
                        writeln!(output, "    ... and {} more traits", traits.len() - 5)?;
                    }
                }

                // Show top functions
                let functions = crate_index.public_functions();
                if !functions.is_empty() {
                    writeln!(output, "  Functions:")?;
                    for item in functions.iter().take(5) {
                        if item.name.is_some() {
                            let path = crate_index.get_item_path(item);
                            writeln!(output, "    • {}", path)?;
                        }
                    }
                    if functions.len() > 5 {
                        writeln!(output, "    ... and {} more functions", functions.len() - 5)?;
                    }
                }
            }
        }
        Err(e) => {
            writeln!(output, "\nDocumentation: Not available")?;
            writeln!(output, "  Error: {}", e)?;
        }
    }

    Ok(output)
}

/// Count items by kind in a crate
fn count_items_by_kind(crate_index: &crate::search::CrateIndex) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for item in crate_index.index.values() {
        let kind = match &item.inner {
            ItemEnum::Module(_) => "Modules",
            ItemEnum::Struct(_) => "Structs",
            ItemEnum::Enum(_) => "Enums",
            ItemEnum::Function(_) => "Functions",
            ItemEnum::Trait(_) => "Traits",
            ItemEnum::TypeAlias(_) => "Type Aliases",
            ItemEnum::Constant { .. } => "Constants",
            ItemEnum::Static(_) => "Statics",
            ItemEnum::Macro(_) => "Macros",
            _ => continue,
        };

        *counts.entry(kind.to_string()).or_insert(0) += 1;
    }

    // Sort by count descending
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|(_, count_a), (_, count_b)| count_b.cmp(count_a));

    sorted.into_iter().collect()
}

/// Truncate description to a maximum length, breaking at word boundaries
fn truncate_description(desc: &str, max_len: usize) -> String {
    let first_line = desc.lines().next().unwrap_or(desc);

    if first_line.len() <= max_len {
        return first_line.to_string();
    }

    // Find last space before max_len
    if let Some(pos) = first_line[..max_len].rfind(' ') {
        format!("{}...", &first_line[..pos])
    } else {
        format!("{}...", &first_line[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ServerContext;
    use crate::workspace::{CrateMetadata, WorkspaceContext};
    use assert2::{check, let_assert};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_inspect_crate_no_workspace() {
        let context = ServerContext::new();
        let request = InspectCrateRequest {
            crate_name: None,
            detail_level: DetailLevel::Medium,
        };

        let result = handle_inspect_crate(&context, request).await;
        let_assert!(Err(err) = result);
        check!(err.to_string().contains("Workspace not configured"));
    }

    #[tokio::test]
    async fn test_inspect_crate_summary_mode() {
        let mut context = ServerContext::new();

        let mut crate_info = HashMap::new();
        crate_info.insert(
            "my-crate".to_string(),
            CrateMetadata {
                origin: CrateOrigin::Local,
                version: Some("0.1.0".to_string()),
                description: Some("Test crate".to_string()),
                dev_dep: false,
                name: "my-crate".to_string(),
                is_root_crate: true,
                used_by: vec![],
            },
        );
        crate_info.insert(
            "serde".to_string(),
            CrateMetadata {
                origin: CrateOrigin::External,
                version: Some("1.0.0".to_string()),
                description: Some("Serialization framework".to_string()),
                dev_dep: false,
                name: "serde".to_string(),
                is_root_crate: false,
                used_by: vec!["my-crate".to_string()],
            },
        );
        crate_info.insert(
            "tokio".to_string(),
            CrateMetadata {
                origin: CrateOrigin::External,
                version: Some("1.0.0".to_string()),
                description: Some("Async runtime".to_string()),
                dev_dep: false,
                name: "tokio".to_string(),
                is_root_crate: false,
                used_by: vec!["my-crate".to_string()],
            },
        );

        let workspace_ctx = WorkspaceContext {
            root: PathBuf::from("/test/project"),
            members: vec!["my-crate".to_string()],
            crate_info,
            root_crate: Some("my-crate".to_string()),
        };

        context.set_workspace_context(workspace_ctx);

        let request = InspectCrateRequest {
            crate_name: None,
            detail_level: DetailLevel::High,
        };

        let result = handle_inspect_crate(&context, request).await.unwrap();

        check!(result.contains("Workspace Members (1)"));
        check!(result.contains("my-crate"));
        check!(result.contains("External Dependencies (2)"));
        check!(result.contains("serde"));
        check!(result.contains("tokio"));
        check!(result.contains("Serialization framework"));
    }

    #[tokio::test]
    async fn test_inspect_crate_detail_mode_not_found() {
        let mut context = ServerContext::new();

        let workspace_ctx = WorkspaceContext {
            root: PathBuf::from("/test/project"),
            members: vec!["my-crate".to_string()],
            crate_info: HashMap::new(),
            root_crate: Some("my-crate".to_string()),
        };

        context.set_workspace_context(workspace_ctx);

        let request = InspectCrateRequest {
            crate_name: Some("nonexistent".to_string()),
            detail_level: DetailLevel::Medium,
        };

        let result = handle_inspect_crate(&context, request).await;
        let_assert!(Err(err) = result);
        check!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_truncate_description() {
        check!(truncate_description("short", 100) == "short");
        check!(
            truncate_description("a very long description that exceeds the limit", 20)
                == "a very long..."
        );
        check!(truncate_description("exact length test", 17) == "exact length test");
    }
}
