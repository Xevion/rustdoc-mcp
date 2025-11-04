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

    // Resolve crate name from path
    let crate_name = if let Some(resolved) = resolve_crate_from_path(&mut path, &known_crates) {
        resolved
    } else if path.path_components.len() == 1 {
        // Single component query without crate - search in workspace members first
        if let Some(member) = workspace_meta.members.first() {
            member.clone()
        } else {
            return Err("No workspace members found. Use set_workspace first.".to_string());
        }
    } else {
        // Multi-component path without resolved crate - try first workspace member
        if let Some(member) = workspace_meta.members.first() {
            member.clone()
        } else {
            return Err("No workspace members found. Use set_workspace first.".to_string());
        }
    };

    // Get version if it's a dependency
    let version = workspace_meta
        .dependencies
        .iter()
        .find(|(name, _)| name == &crate_name)
        .map(|(_, ver)| ver.as_str());

    // Load documentation
    let doc = get_docs(&crate_name, version, workspace_root).map_err(|e| {
        format!(
            "Failed to load documentation for '{}': {}",
            crate_name, e
        )
    })?;

    // Search for the item
    let search_query = path.full_path();
    let results = doc.search_with_filter(&search_query, request.kind);

    if results.is_empty() {
        return Err(format!(
            "No items found matching '{}' in crate '{}'{}",
            search_query,
            crate_name,
            if let Some(k) = request.kind {
                format!(" with kind '{:?}'", k)
            } else {
                String::new()
            }
        ));
    }

    // Handle multiple matches - error with suggestions
    if results.len() > 1 {
        return Err(format_disambiguation_error(&results, &search_query, &crate_name));
    }

    // Single match found - format the output
    let result = &results[0];
    let item = doc
        .get_item(result.id.as_ref().unwrap())
        .ok_or_else(|| "Item not found in documentation index".to_string())?;

    format_item_output(item, &doc, request.verbosity, &crate_name)
}

/// Format a disambiguation error when multiple items match
fn format_disambiguation_error(
    results: &[SearchResult],
    query: &str,
    crate_name: &str,
) -> String {
    let mut error = format!(
        "Multiple items found matching '{}' in '{}'. Please be more specific:\n\n",
        query, crate_name
    );

    for (i, result) in results.iter().enumerate().take(10) {
        let _ = writeln!(
            &mut error,
            "{}. {} [{}] - {}",
            i + 1,
            result.path,
            result.kind,
            result.docs.as_deref().unwrap_or("No documentation")
                .lines()
                .next()
                .unwrap_or("")
        );
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
