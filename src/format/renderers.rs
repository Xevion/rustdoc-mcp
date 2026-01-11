//! Final output rendering for documentation display.
//!
//! This module handles the rendering of rustdoc items into human-readable text output,
//! including documentation formatting, signature display, and detail level control.

use super::{DetailLevel, TypeFormatter};
use crate::item::item_ref::ItemRef;
use rustdoc_types::{Item, ItemEnum, ItemKind};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Render struct output
pub(crate) fn render_struct(
    output: &mut String,
    item: ItemRef<'_, Item>,
    s: &rustdoc_types::Struct,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    // Low: signature only
    let _ = writeln!(output, "struct {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    // High: add fields and implementations
    if matches!(detail_level, DetailLevel::High) {
        let _ = writeln!(output, "\nFields:");
        match &s.kind {
            rustdoc_types::StructKind::Plain { fields, .. } => {
                for field_id in fields {
                    if let Some(field_item) = item.get(field_id)
                        && let ItemEnum::StructField(ty) = field_item.inner()
                    {
                        let field_name = field_item.name().unwrap_or("<unnamed>");
                        let type_name = item.crate_index().format_type(ty);
                        let _ = writeln!(output, "  {}: {}", field_name, type_name);
                    }
                }
            }
            rustdoc_types::StructKind::Tuple(fields) => {
                for (i, field_id_opt) in fields.iter().enumerate() {
                    if let Some(field_id) = field_id_opt
                        && let Some(field_item) = item.get(field_id)
                        && let ItemEnum::StructField(ty) = field_item.inner()
                    {
                        let type_name = item.crate_index().format_type(ty);
                        let _ = writeln!(output, "  {}: {}", i, type_name);
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

/// Render enum output
pub(crate) fn render_enum(
    output: &mut String,
    item: ItemRef<'_, Item>,
    e: &rustdoc_types::Enum,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    // Low: signature only
    let _ = writeln!(output, "enum {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    // High: add variants
    if matches!(detail_level, DetailLevel::High) {
        let _ = writeln!(output, "\nVariants:");
        for variant_id in &e.variants {
            if let Some(variant_item) = item.get(variant_id)
                && let ItemEnum::Variant(v) = variant_item.inner()
            {
                let variant_name = variant_item.name().unwrap_or("<unnamed>");
                match &v.kind {
                    rustdoc_types::VariantKind::Plain => {
                        let _ = writeln!(output, "  {},", variant_name);
                    }
                    rustdoc_types::VariantKind::Tuple(fields) => {
                        let _ = write!(output, "  {}(", variant_name);
                        for (i, field_id_opt) in fields.iter().enumerate() {
                            if let Some(field_id) = field_id_opt
                                && let Some(field_item) = item.get(field_id)
                                && let ItemEnum::StructField(ty) = field_item.inner()
                            {
                                if i > 0 {
                                    let _ = write!(output, ", ");
                                }
                                let _ = write!(output, "{}", item.crate_index().format_type(ty));
                            }
                        }
                        let _ = writeln!(output, "),");
                    }
                    rustdoc_types::VariantKind::Struct { fields, .. } => {
                        let _ = writeln!(output, "  {} {{", variant_name);
                        for field_id in fields {
                            if let Some(field_item) = item.get(field_id)
                                && let ItemEnum::StructField(ty) = field_item.inner()
                            {
                                let field_name = field_item.name().unwrap_or("<unnamed>");
                                let _ = writeln!(
                                    output,
                                    "    {}: {},",
                                    field_name,
                                    item.crate_index().format_type(ty)
                                );
                            }
                        }
                        let _ = writeln!(output, "  }},");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Render function output
pub(crate) fn render_function(
    output: &mut String,
    item: ItemRef<'_, Item>,
    _f: &rustdoc_types::Function,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    // Low: signature only
    if let Some(signature) = item.crate_index().format_function_signature(&item) {
        let _ = writeln!(output, "{}", signature);
    } else {
        let _ = writeln!(output, "fn {}()", name);
    }
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    Ok(())
}

/// Render trait output
pub(crate) fn render_trait(
    output: &mut String,
    item: ItemRef<'_, Item>,
    t: &rustdoc_types::Trait,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    // Low: signature only
    let _ = writeln!(output, "trait {} {{", name);
    let _ = writeln!(output, "  // in {}::{}", crate_name, path);
    let _ = writeln!(output, "}}");

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    // High: add methods
    if matches!(detail_level, DetailLevel::High) {
        let _ = writeln!(output, "\nMethods:");
        for item_id in &t.items {
            if let Some(method_item) = item.get(item_id)
                && matches!(method_item.inner(), ItemEnum::Function(_))
                && let Some(sig) = item.crate_index().format_function_signature(&method_item)
            {
                let _ = writeln!(output, "  {}", sig);
            }
        }
    }

    Ok(())
}

/// Render module output
pub(crate) fn render_module(
    output: &mut String,
    item: ItemRef<'_, Item>,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let default_name = crate_name.to_string();
    let name = item.name().unwrap_or(&default_name);
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    let _ = writeln!(output, "module {}", name);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    // Get module's child items
    let children: Vec<_> = item.children().build().collect();

    // Show docs for medium/high
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    // Determine item limit based on detail level
    let item_limit = match detail_level {
        DetailLevel::Low => 4,
        DetailLevel::Medium => 10,
        DetailLevel::High => usize::MAX, // unlimited
    };

    // Categorize items by kind
    let mut groups: HashMap<ItemKind, Vec<ItemRef<'_, rustdoc_types::Item>>> = HashMap::new();

    for child in children {
        groups.entry(child.kind()).or_default().push(child);
    }

    // Display in order
    const CATEGORY_ORDER: &[(ItemKind, &str)] = &[
        (ItemKind::Module, "Modules"),
        (ItemKind::Struct, "Structs"),
        (ItemKind::Enum, "Enums"),
        (ItemKind::Trait, "Traits"),
        (ItemKind::Union, "Unions"),
        (ItemKind::TypeAlias, "Type Aliases"),
        (ItemKind::Function, "Functions"),
        (ItemKind::Constant, "Constants"),
        (ItemKind::Static, "Statics"),
        (ItemKind::Macro, "Macros"),
    ];

    for (kind, category_name) in CATEGORY_ORDER {
        if let Some(items) = groups.get(kind) {
            if items.is_empty() {
                continue;
            }

            let _ = writeln!(output, "\n{}:", category_name);
            let displayed_count = items.len().min(item_limit);

            for child in items.iter().take(displayed_count) {
                let child_name = child.name().unwrap_or("<unnamed>");

                match detail_level {
                    DetailLevel::Low => {
                        // Just the name
                        let _ = writeln!(output, "  {}", child_name);
                    }
                    DetailLevel::Medium => {
                        // Name + first line of docs
                        let _ = write!(output, "  {}", child_name);
                        if let Some(docs) = child.comment()
                            && let Some(first_line) = docs.lines().next()
                        {
                            let trimmed = first_line.trim();
                            if !trimmed.is_empty() {
                                let _ = write!(output, " // {}", trimmed);
                            }
                        }
                        let _ = writeln!(output);
                    }
                    DetailLevel::High => {
                        // Signature + summary docs
                        if let Some(sig) = render_item_signature(*child) {
                            let _ = writeln!(output, "  {}", sig);
                        } else {
                            let _ = writeln!(output, "  {}", child_name);
                        }

                        // Add summary index comment
                        if let Some(docs) = child.comment()
                            && let Some(first_line) = docs.lines().next()
                        {
                            let trimmed = first_line.trim();
                            if !trimmed.is_empty() {
                                let _ = writeln!(output, "    // {}", trimmed);
                            }
                        }
                    }
                }
            }

            // Show count if we hit the limit
            if items.len() > displayed_count {
                let _ = writeln!(output, "  ... and {} more", items.len() - displayed_count);
            }
        }
    }

    Ok(())
}

/// Render type alias output
pub(crate) fn render_type_alias(
    output: &mut String,
    item: ItemRef<'_, Item>,
    ta: &rustdoc_types::TypeAlias,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let type_str = item.crate_index().format_type(&ta.type_);

    let _ = writeln!(output, "type {} = {};", name, type_str);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    Ok(())
}

/// Render constant output
pub(crate) fn render_constant(
    output: &mut String,
    item: ItemRef<'_, Item>,
    type_: &rustdoc_types::Type,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let type_str = item.crate_index().format_type(type_);

    let _ = writeln!(output, "const {}: {};", name, type_str);
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    Ok(())
}

/// Render static output
pub(crate) fn render_static(
    output: &mut String,
    item: ItemRef<'_, Item>,
    s: &rustdoc_types::Static,
    detail_level: DetailLevel,
    crate_name: &str,
) -> Result<(), String> {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let type_str = item.crate_index().format_type(&s.type_);

    let _ = writeln!(
        output,
        "static {}{}: {};",
        if s.is_mutable { "mut " } else { "" },
        name,
        type_str
    );
    let _ = writeln!(output, "// in {}::{}", crate_name, path);

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        let _ = writeln!(output, "\n{}", short_docs);
    }

    Ok(())
}

/// Generate a signature string for an item
pub(crate) fn render_item_signature<'a>(
    item: crate::item::ItemRef<'a, rustdoc_types::Item>,
) -> Option<String> {
    let name = item.name()?;

    match item.inner() {
        ItemEnum::Function(_) => item.crate_index().format_function_signature(&item),
        ItemEnum::Struct(_) => Some(format!("struct {}", name)),
        ItemEnum::Enum(_) => Some(format!("enum {}", name)),
        ItemEnum::Trait(_) => Some(format!("trait {}", name)),
        ItemEnum::TypeAlias(ta) => {
            let type_str = item.crate_index().format_type(&ta.type_);
            Some(format!("type {} = {}", name, type_str))
        }
        ItemEnum::Constant { type_, .. } => {
            let type_str = item.crate_index().format_type(type_);
            Some(format!("const {}: {}", name, type_str))
        }
        ItemEnum::Static(s) => {
            let type_str = item.crate_index().format_type(&s.type_);
            Some(format!(
                "static {}{}: {}",
                if s.is_mutable { "mut " } else { "" },
                name,
                type_str
            ))
        }
        ItemEnum::Module(_) => Some(format!("mod {}", name)),
        ItemEnum::Macro(_) => Some(format!("macro {}", name)),
        _ => None,
    }
}

/// Extract documentation summary (first paragraph) for truncated output
fn extract_summary(docs: &str) -> String {
    docs.split("\n\n").next().unwrap_or(docs).trim().to_string()
}
