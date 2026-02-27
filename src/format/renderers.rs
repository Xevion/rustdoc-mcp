//! Final output rendering for documentation display.
//!
//! This module handles the rendering of rustdoc items into human-readable text output,
//! including documentation formatting, signature display, and detail level control.

use super::{DetailLevel, TypeFormatter};
use crate::item::item_ref::ItemRef;
use rustdoc_types::{Item, ItemEnum, ItemKind};
use std::collections::HashMap;
use std::fmt::{self, Write as _};

/// Render struct output
pub(crate) fn render_struct(
    output: &mut String,
    item: ItemRef<'_, Item>,
    s: &rustdoc_types::Struct,
    detail_level: DetailLevel,
    crate_name: &str,
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    // Low: signature with generics
    write!(output, "struct {}", name)?;
    fmt.write_generics(output, &s.generics)?;

    let sig_len = 7 + name.len(); // Approximate for "struct " + name
    fmt.write_where_clause(output, &s.generics.where_predicates, sig_len)?;

    writeln!(output, " {{")?;
    writeln!(output, "  // in {}::{}", crate_name, path)?;
    writeln!(output, "}}")?;

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
    }

    // High: add fields and implementations
    if matches!(detail_level, DetailLevel::High) {
        writeln!(output, "\nFields:")?;
        match &s.kind {
            rustdoc_types::StructKind::Plain {
                fields,
                has_stripped_fields,
            } => {
                // TODO: when `has_stripped_fields` is true and no public fields are rendered,
                // emit a note like "(N private fields not shown)" so the caller knows why
                // Fields: is empty. Currently an all-private struct silently shows nothing.
                let _ = has_stripped_fields;
                for field_id in fields {
                    if let Some(field_item) = item.get(field_id)
                        && let ItemEnum::StructField(ty) = field_item.inner()
                    {
                        let field_name = field_item.name().unwrap_or("<unnamed>");
                        write!(output, "  {}: ", field_name)?;
                        fmt.write_type(output, ty)?;
                        writeln!(output)?;
                    }
                }
            }
            rustdoc_types::StructKind::Tuple(fields) => {
                for (i, field_id_opt) in fields.iter().enumerate() {
                    if let Some(field_id) = field_id_opt
                        && let Some(field_item) = item.get(field_id)
                        && let ItemEnum::StructField(ty) = field_item.inner()
                    {
                        write!(output, "  {}: ", i)?;
                        fmt.write_type(output, ty)?;
                        writeln!(output)?;
                    }
                }
            }
            rustdoc_types::StructKind::Unit => {
                writeln!(output, "  (unit struct)")?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    // Low: signature with generics
    write!(output, "enum {}", name)?;
    fmt.write_generics(output, &e.generics)?;

    let sig_len = 5 + name.len(); // Approximate for "enum " + name
    fmt.write_where_clause(output, &e.generics.where_predicates, sig_len)?;

    writeln!(output, " {{")?;
    writeln!(output, "  // in {}::{}", crate_name, path)?;
    writeln!(output, "}}")?;

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
    }

    // High: add variants
    if matches!(detail_level, DetailLevel::High) {
        writeln!(output, "\nVariants:")?;
        for variant_id in &e.variants {
            if let Some(variant_item) = item.get(variant_id)
                && let ItemEnum::Variant(v) = variant_item.inner()
            {
                let variant_name = variant_item.name().unwrap_or("<unnamed>");
                match &v.kind {
                    rustdoc_types::VariantKind::Plain => {
                        writeln!(output, "  {},", variant_name)?;
                    }
                    rustdoc_types::VariantKind::Tuple(fields) => {
                        write!(output, "  {}(", variant_name)?;
                        for (i, field_id_opt) in fields.iter().enumerate() {
                            if let Some(field_id) = field_id_opt
                                && let Some(field_item) = item.get(field_id)
                                && let ItemEnum::StructField(ty) = field_item.inner()
                            {
                                if i > 0 {
                                    write!(output, ", ")?;
                                }
                                fmt.write_type(output, ty)?;
                            }
                        }
                        writeln!(output, "),")?;
                    }
                    rustdoc_types::VariantKind::Struct { fields, .. } => {
                        writeln!(output, "  {} {{", variant_name)?;
                        for field_id in fields {
                            if let Some(field_item) = item.get(field_id)
                                && let ItemEnum::StructField(ty) = field_item.inner()
                            {
                                let field_name = field_item.name().unwrap_or("<unnamed>");
                                write!(output, "    {}: ", field_name)?;
                                fmt.write_type(output, ty)?;
                                writeln!(output, ",")?;
                            }
                        }
                        writeln!(output, "  }},")?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    // Low: signature only
    fmt.write_function_signature(output, &item)?;
    writeln!(output)?;
    writeln!(output, "// in {}::{}", crate_name, path)?;

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    // Low: signature with generics and supertraits
    write!(output, "trait {}", name)?;
    fmt.write_generics(output, &t.generics)?;

    let supertrait_len = 6 + name.len(); // Approximate for "trait " + name
    fmt.write_supertrait_bounds(output, &t.bounds, supertrait_len)?;

    let sig_len = supertrait_len; // Approximate
    fmt.write_where_clause(output, &t.generics.where_predicates, sig_len)?;

    writeln!(output, " {{")?;
    writeln!(output, "  // in {}::{}", crate_name, path)?;
    writeln!(output, "}}")?;

    // Medium: add short docs
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
    }

    // High: add methods
    if matches!(detail_level, DetailLevel::High) {
        writeln!(output, "\nMethods:")?;
        for item_id in &t.items {
            if let Some(method_item) = item.get(item_id)
                && matches!(method_item.inner(), ItemEnum::Function(_))
            {
                write!(output, "  ")?;
                fmt.write_function_signature(output, &method_item)?;
                writeln!(output)?;
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
) -> fmt::Result {
    let default_name = crate_name.to_string();
    let name = item.name().unwrap_or(&default_name);
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());

    writeln!(output, "module {}", name)?;
    writeln!(output, "// in {}::{}", crate_name, path)?;

    // Get module's child items
    let children: Vec<_> = item.children().build().collect();

    // Show docs for medium/high
    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
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

            writeln!(output, "\n{}:", category_name)?;
            let displayed_count = items.len().min(item_limit);

            for child in items.iter().take(displayed_count) {
                let child_name = child.name().unwrap_or("<unnamed>");

                match detail_level {
                    DetailLevel::Low => {
                        // Just the name
                        writeln!(output, "  {}", child_name)?;
                    }
                    DetailLevel::Medium => {
                        // Name + first line of docs
                        write!(output, "  {}", child_name)?;
                        if let Some(docs) = child.comment()
                            && let Some(first_line) = docs.lines().next()
                        {
                            let trimmed = first_line.trim();
                            if !trimmed.is_empty() {
                                write!(output, " // {}", trimmed)?;
                            }
                        }
                        writeln!(output)?;
                    }
                    DetailLevel::High => {
                        // Signature + summary docs
                        if let Some(sig) = render_item_signature(*child) {
                            writeln!(output, "  {}", sig)?;
                        } else {
                            writeln!(output, "  {}", child_name)?;
                        }

                        // Add summary index comment
                        if let Some(docs) = child.comment()
                            && let Some(first_line) = docs.lines().next()
                        {
                            let trimmed = first_line.trim();
                            if !trimmed.is_empty() {
                                writeln!(output, "    // {}", trimmed)?;
                            }
                        }
                    }
                }
            }

            // Show count if we hit the limit
            if items.len() > displayed_count {
                writeln!(output, "  ... and {} more", items.len() - displayed_count)?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    write!(output, "type {}", name)?;
    fmt.write_generics(output, &ta.generics)?;
    write!(output, " = ")?;
    fmt.write_type(output, &ta.type_)?;
    writeln!(output, ";")?;
    writeln!(output, "// in {}::{}", crate_name, path)?;

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    write!(output, "const {}: ", name)?;
    fmt.write_type(output, type_)?;
    writeln!(output, ";")?;
    writeln!(output, "// in {}::{}", crate_name, path)?;

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
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
) -> fmt::Result {
    let name = item.name().unwrap_or("<unnamed>");
    let path = item
        .path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string());
    let fmt = TypeFormatter::new(item.crate_index());

    write!(
        output,
        "static {}{}: ",
        if s.is_mutable { "mut " } else { "" },
        name
    )?;
    fmt.write_type(output, &s.type_)?;
    writeln!(output, ";")?;
    writeln!(output, "// in {}::{}", crate_name, path)?;

    if matches!(detail_level, DetailLevel::Medium | DetailLevel::High)
        && let Some(docs) = item.comment()
    {
        let short_docs = extract_summary(docs);
        writeln!(output, "\n{}", short_docs)?;
    }

    Ok(())
}

/// Generate a signature string for an item
pub(crate) fn render_item_signature<'a>(
    item: crate::item::ItemRef<'a, rustdoc_types::Item>,
) -> Option<String> {
    let name = item.name()?;
    let fmt = TypeFormatter::new(item.crate_index());
    let mut s = String::new();

    let result = match item.inner() {
        ItemEnum::Function(_) => fmt.write_function_signature(&mut s, &item),
        ItemEnum::Struct(st) => {
            write!(&mut s, "struct {}", name).ok()?;
            fmt.write_generics(&mut s, &st.generics)
        }
        ItemEnum::Enum(e) => {
            write!(&mut s, "enum {}", name).ok()?;
            fmt.write_generics(&mut s, &e.generics)
        }
        ItemEnum::Trait(t) => {
            write!(&mut s, "trait {}", name).ok()?;
            fmt.write_generics(&mut s, &t.generics).ok()?;
            let len = 6 + name.len(); // Approximate
            fmt.write_supertrait_bounds(&mut s, &t.bounds, len)
        }
        ItemEnum::TypeAlias(ta) => {
            write!(&mut s, "type {}", name).ok()?;
            fmt.write_generics(&mut s, &ta.generics).ok()?;
            write!(&mut s, " = ").ok()?;
            fmt.write_type(&mut s, &ta.type_)
        }
        ItemEnum::Constant { type_, .. } => {
            write!(&mut s, "const {}: ", name).ok()?;
            fmt.write_type(&mut s, type_)
        }
        ItemEnum::Static(st) => {
            write!(
                &mut s,
                "static {}{}: ",
                if st.is_mutable { "mut " } else { "" },
                name
            )
            .ok()?;
            fmt.write_type(&mut s, &st.type_)
        }
        ItemEnum::Module(_) => {
            write!(&mut s, "mod {}", name)
        }
        ItemEnum::Macro(_) => {
            write!(&mut s, "macro {}", name)
        }
        _ => return None,
    };

    result.ok().map(|_| s)
}

/// Extract documentation summary (first paragraph) for truncated output
fn extract_summary(docs: &str) -> String {
    docs.split("\n\n").next().unwrap_or(docs).trim().to_string()
}
