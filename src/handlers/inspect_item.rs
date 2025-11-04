use crate::cargo::get_docs;
use crate::context::ServerContext;
use crate::doc::DocIndex;
use crate::path::{parse_item_path, resolve_crate_from_path};
use crate::types::{ItemKind, SearchResult};
use rmcp::schemars;
use rustdoc_types::{Item, ItemEnum};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InspectItemRequest {
    /// Item to inspect (e.g., "Vec", "std::vec::Vec", "HashMap")
    pub query: String,
    /// Optional filter by item kind (struct, enum, function, trait, module, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ItemKind>,
    /// Verbosity level: minimal (signature only), brief (+docs), full (+members+impls)
    #[serde(default = "default_verbosity")]
    pub verbosity: VerbosityLevel,
}

fn default_verbosity() -> VerbosityLevel {
    VerbosityLevel::Brief
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerbosityLevel {
    Minimal,
    Brief,
    Full,
}

/// Execute the inspect_item operation
pub fn execute_inspect_item(
    context: &ServerContext,
    request: InspectItemRequest,
) -> Result<String, String> {
    // Parse the item path
    let mut path = parse_item_path(&request.query);

    // Get available crates from workspace context
    let workspace_meta = context
        .workspace_metadata()
        .ok_or_else(|| "No workspace configured. Use set_workspace first.".to_string())?;

    // Get workspace root directory
    let workspace_root = context
        .working_directory()
        .ok_or_else(|| "No working directory configured. Use set_workspace first.".to_string())?;

    // Build list of known crates (members + dependencies)
    let mut known_crates = workspace_meta.members.clone();
    known_crates.extend(
        workspace_meta
            .dependencies
            .iter()
            .map(|(name, _)| name.clone()),
    );

    // Track if this was originally a path query (before crate resolution)
    let is_path_query = path.path_components.len() > 1 || request.query.contains("::");

    // Check if the query specifies a crate (e.g., "serde::Serialize")
    let specified_crate = resolve_crate_from_path(&mut path, &known_crates);
    let search_query = path.full_path();

    // Determine which crates to search
    let crates_to_search: Vec<String> = if let Some(crate_name) = specified_crate {
        // User specified a crate - only search that one
        vec![crate_name]
    } else {
        // No crate specified - search workspace members + all direct dependencies
        let mut crates = workspace_meta.members.clone();
        crates.extend(workspace_meta.dependencies.iter().map(|(name, _)| name.clone()));
        crates
    };

    // Search across all target crates
    let mut all_results = Vec::new();
    let cargo_lock_path = context.cargo_lock_path().map(|p| p.as_path());

    for crate_name in &crates_to_search {
        // Determine if this is a workspace member
        let is_workspace_member = workspace_meta.members.contains(&crate_name.to_string());

        // Get version if it's a dependency
        let version = workspace_meta
            .dependencies
            .iter()
            .find(|(name, _)| name == crate_name)
            .map(|(_, ver)| ver.as_str());

        // Load documentation for this crate
        let doc = match get_docs(crate_name, version, workspace_root, is_workspace_member, cargo_lock_path) {
            Ok(d) => d,
            Err(_) => continue, // Skip crates we can't load docs for
        };

        // Search within this crate
        let mut results = doc.search_with_filter_ex(&search_query, request.kind, is_path_query);

        // Mark each result with the source crate
        for result in &mut results {
            result.source_crate = Some(crate_name.clone());
        }

        all_results.extend(results);
    }

    // Sort results by relevance
    all_results.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then_with(|| a.name.cmp(&b.name))
    });

    if all_results.is_empty() {
        return Err(format!(
            "No items found matching '{}'{}",
            search_query,
            if let Some(k) = request.kind {
                format!(" with kind '{:?}'", k)
            } else {
                String::new()
            }
        ));
    }

    // Handle multiple matches - error with suggestions
    if all_results.len() > 1 {
        return Err(format_disambiguation_error(
            &all_results,
            &search_query,
            crates_to_search.first().unwrap(),
        ));
    }

    // Single match found - load the specific crate's docs and format output
    let result = &all_results[0];

    // Use the actual crate where the item is defined (may be different from search crate)
    let item_crate = result.crate_name.as_ref()
        .or(result.source_crate.as_ref())
        .ok_or_else(|| "No crate information for matched item".to_string())?;

    let is_workspace_member = workspace_meta.members.contains(&item_crate.to_string());

    let version = workspace_meta
        .dependencies
        .iter()
        .find(|(name, _)| name == item_crate)
        .map(|(_, ver)| ver.as_str());

    let doc = get_docs(item_crate, version, workspace_root, is_workspace_member, cargo_lock_path).map_err(|e| {
        format!(
            "Failed to load documentation for '{}': {}",
            item_crate, e
        )
    })?;

    let item_id = result.id.as_ref().ok_or_else(|| {
        format!(
            "Item '{}' ({}) at '{}' has no ID in search results",
            result.name, result.kind, result.path
        )
    })?;
    let item = doc.get_item(item_id).ok_or_else(|| {
        format!(
            "Item '{}' ({}) found at '{}' but documentation not loaded",
            result.name, result.kind, result.path
        )
    })?;

    format_item_output(item, &doc, request.verbosity, item_crate)
}

/// Format a disambiguation error when multiple items match
fn format_disambiguation_error(
    results: &[SearchResult],
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
            if !docs_trimmed.is_empty() {
                if let Some(first_line) = docs_trimmed.lines().next() {
                    let first_line_trimmed = first_line.trim();
                    if !first_line_trimmed.is_empty() {
                        let _ = write!(&mut error, " - {}", first_line_trimmed);
                    }
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
    item: &Item,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<String, String> {
    let mut output = String::new();

    let result = match &item.inner {
        ItemEnum::Struct(s) => format_struct_output(&mut output, item, s, doc, verbosity, crate_name),
        ItemEnum::Enum(e) => format_enum_output(&mut output, item, e, doc, verbosity, crate_name),
        ItemEnum::Function(f) => format_function_output(&mut output, item, f, doc, verbosity, crate_name),
        ItemEnum::Trait(t) => format_trait_output(&mut output, item, t, doc, verbosity, crate_name),
        ItemEnum::Module(_) => format_module_output(&mut output, item, doc, verbosity, crate_name),
        ItemEnum::TypeAlias(ta) => format_type_alias_output(&mut output, item, ta, doc, verbosity, crate_name),
        ItemEnum::Constant { type_, const_: _ } => format_constant_output(&mut output, item, type_, doc, verbosity, crate_name),
        ItemEnum::Static(s) => format_static_output(&mut output, item, s, doc, verbosity, crate_name),
        _ => {
            Err(format!(
                "Unsupported item type: {:?}",
                &item.inner
            ))
        }
    };

    result?;
    Ok(output)
}

/// Format struct output
fn format_struct_output(
    output: &mut String,
    item: &Item,
    s: &rustdoc_types::Struct,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);

    // Minimal: signature only
    let _ = writeln!(output, "struct {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Brief: add short docs
    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    // Full: add fields and implementations
    if matches!(verbosity, VerbosityLevel::Full) {
        let _ = writeln!(output, "\nFields:");
        match &s.kind {
            rustdoc_types::StructKind::Plain { fields, .. } => {
                for field_id in fields {
                    if let Some(field_item) = doc.get_item(field_id) {
                        if let ItemEnum::StructField(ty) = &field_item.inner {
                            let field_name = field_item.name.as_deref().unwrap_or("<unnamed>");
                            let type_name = doc.format_type(ty);
                            let _ = writeln!(output, "  {}: {}", field_name, type_name);
                        }
                    }
                }
            }
            rustdoc_types::StructKind::Tuple(fields) => {
                for (i, field_id_opt) in fields.iter().enumerate() {
                    if let Some(field_id) = field_id_opt {
                        if let Some(field_item) = doc.get_item(field_id) {
                            if let ItemEnum::StructField(ty) = &field_item.inner {
                                let type_name = doc.format_type(ty);
                                let _ = writeln!(output, "  {}: {}", i, type_name);
                            }
                        }
                    }
                }
            }
            rustdoc_types::StructKind::Unit => {
                let _ = writeln!(output, "  (unit struct)");
            }
        }
    }

    Ok(())
}

/// Format enum output
fn format_enum_output(
    output: &mut String,
    item: &Item,
    e: &rustdoc_types::Enum,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);

    // Minimal: signature only
    let _ = writeln!(output, "enum {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Brief: add short docs
    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    // Full: add variants
    if matches!(verbosity, VerbosityLevel::Full) {
        let _ = writeln!(output, "\nVariants:");
        for variant_id in &e.variants {
            if let Some(variant_item) = doc.get_item(variant_id) {
                if let ItemEnum::Variant(v) = &variant_item.inner {
                    let variant_name = variant_item.name.as_deref().unwrap_or("<unnamed>");
                    match &v.kind {
                        rustdoc_types::VariantKind::Plain => {
                            let _ = writeln!(output, "  {},", variant_name);
                        }
                        rustdoc_types::VariantKind::Tuple(fields) => {
                            let _ = write!(output, "  {}(", variant_name);
                            for (i, field_id_opt) in fields.iter().enumerate() {
                                if let Some(field_id) = field_id_opt {
                                    if let Some(field_item) = doc.get_item(field_id) {
                                        if let ItemEnum::StructField(ty) = &field_item.inner {
                                            if i > 0 {
                                                let _ = write!(output, ", ");
                                            }
                                            let _ = write!(output, "{}", doc.format_type(ty));
                                        }
                                    }
                                }
                            }
                            let _ = writeln!(output, "),");
                        }
                        rustdoc_types::VariantKind::Struct { fields, .. } => {
                            let _ = writeln!(output, "  {} {{", variant_name);
                            for field_id in fields {
                                if let Some(field_item) = doc.get_item(field_id) {
                                    if let ItemEnum::StructField(ty) = &field_item.inner {
                                        let field_name =
                                            field_item.name.as_deref().unwrap_or("<unnamed>");
                                        let _ = writeln!(
                                            output,
                                            "    {}: {},",
                                            field_name,
                                            doc.format_type(ty)
                                        );
                                    }
                                }
                            }
                            let _ = writeln!(output, "  }},");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Format function output
fn format_function_output(
    output: &mut String,
    item: &Item,
    _f: &rustdoc_types::Function,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let path = doc.get_item_path(item);

    // Minimal: signature only
    if let Some(signature) = doc.format_function_signature(item) {
        let _ = writeln!(output, "{}", signature);
    } else {
        let name = item.name.as_ref().unwrap();
        let _ = writeln!(output, "fn {}()", name);
    }
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    // Brief: add short docs
    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    Ok(())
}

/// Format trait output
fn format_trait_output(
    output: &mut String,
    item: &Item,
    t: &rustdoc_types::Trait,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);

    // Minimal: signature only
    let _ = writeln!(output, "trait {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Brief: add short docs
    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    // Full: add methods
    if matches!(verbosity, VerbosityLevel::Full) {
        let _ = writeln!(output, "\nMethods:");
        for item_id in &t.items {
            if let Some(method_item) = doc.get_item(item_id) {
                if matches!(method_item.inner, ItemEnum::Function(_)) {
                    if let Some(sig) = doc.format_function_signature(method_item) {
                        let _ = writeln!(output, "  {}", sig);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Format module output
fn format_module_output(
    output: &mut String,
    item: &Item,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let default_name = crate_name.to_string();
    let name = item.name.as_ref().unwrap_or(&default_name);
    let path = doc.get_item_path(item);

    let _ = writeln!(output, "module {}", name);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    // Minimal: just list item names
    if matches!(verbosity, VerbosityLevel::Minimal) {
        let _ = writeln!(output, "\nItems:");
        // TODO: List module items
    }

    // Brief: add module docs and categorize
    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
        // TODO: Categorize items by kind
    }

    // Full: add complete docs and signatures
    if matches!(verbosity, VerbosityLevel::Full) {
        // Already showing full docs above
        // TODO: Add full signatures for items
    }

    Ok(())
}

/// Format type alias output
fn format_type_alias_output(
    output: &mut String,
    item: &Item,
    ta: &rustdoc_types::TypeAlias,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);
    let type_str = doc.format_type(&ta.type_);

    let _ = writeln!(output, "type {} = {};", name, type_str);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    Ok(())
}

/// Format constant output
fn format_constant_output(
    output: &mut String,
    item: &Item,
    type_: &rustdoc_types::Type,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);
    let type_str = doc.format_type(type_);

    let _ = writeln!(output, "const {}: {};", name, type_str);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    Ok(())
}

/// Format static output
fn format_static_output(
    output: &mut String,
    item: &Item,
    s: &rustdoc_types::Static,
    doc: &DocIndex,
    verbosity: VerbosityLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name.as_ref().unwrap();
    let path = doc.get_item_path(item);
    let type_str = doc.format_type(&s.type_);

    let _ = writeln!(
        output,
        "static {}{}: {};",
        if s.is_mutable { "mut " } else { "" },
        name,
        type_str
    );
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(verbosity, VerbosityLevel::Brief | VerbosityLevel::Full) {
        if let Some(docs) = &item.docs {
            let short_docs = truncate_docs(docs);
            let _ = writeln!(output, "\n{}", short_docs);
        }
    }

    Ok(())
}

/// Truncate documentation to first paragraph for brief output
fn truncate_docs(docs: &str) -> String {
    docs.split("\n\n")
        .next()
        .unwrap_or(docs)
        .trim()
        .to_string()
}
