//! Building blocks for creating formatted Rust syntax structures.
//!
//! This module handles the generation of Rust syntax structures like type definitions,
//! generics, where clauses, and other low-level building blocks needed for formatting.
//!
//! It also provides type formatting utilities via the `TypeFormatter` trait.

use crate::error::Result;
use crate::format::extraction::TypeInfo;
use crate::search::CrateIndex;
use crate::types::TypeKind;
use anyhow::Context;
use rustdoc_types::{
    GenericBound, GenericParamDef, GenericParamDefKind, Generics, Id, Item, ItemEnum, Type,
    WherePredicate,
};

/// Generate formatted Rust syntax for a type definition using syn + prettyplease
pub fn build_type_syntax(def: &TypeInfo, index: &CrateIndex) -> Result<String> {
    let output = build_unformatted_syntax(def, index)?;

    // Parse and format with prettyplease
    let syntax_tree = syn::parse_file(&output).context("Failed to parse generated Rust syntax")?;
    let formatted = prettyplease::unparse(&syntax_tree);

    Ok(formatted)
}

/// Generate unformatted Rust syntax for a type definition
fn build_unformatted_syntax(def: &TypeInfo, index: &CrateIndex) -> Result<String> {
    let mut output = String::new();

    // Add path comment
    output.push_str(&format!("// From: {}\n", def.path));

    // Add documentation
    if let Some(docs) = &def.docs {
        for line in docs.lines() {
            output.push_str(&format!("/// {}\n", line));
        }
    }

    // Build the type definition based on kind
    match def.kind {
        TypeKind::Struct => {
            output.push_str(&build_struct_definition(def, index)?);
        }
        TypeKind::Enum => {
            output.push_str(&build_enum_definition(def, index)?);
        }
        TypeKind::Union => {
            output.push_str(&build_union_definition(def, index)?);
        }
    }

    Ok(output)
}

fn build_struct_definition(def: &TypeInfo, index: &CrateIndex) -> Result<String> {
    let mut output = String::new();

    output.push_str("pub struct ");
    output.push_str(&def.name);
    output.push_str(&build_generics(&def.generics, index));

    if let Some(where_clause) = build_where_clause(&def.generics.where_predicates, index) {
        output.push('\n');
        output.push_str(&where_clause);
        output.push('\n');
    }

    output.push_str(" {\n");

    if let Some(fields) = &def.fields {
        for field in fields {
            // Add field documentation
            if let Some(field_docs) = &field.docs {
                for line in field_docs.lines() {
                    output.push_str(&format!("    /// {}\n", line));
                }
            }

            output.push_str(&format!(
                "    {} {}: {},\n",
                field.visibility, field.name, field.type_name
            ));
        }
    }

    output.push_str("}\n");

    Ok(output)
}

fn build_enum_definition(def: &TypeInfo, index: &CrateIndex) -> Result<String> {
    let mut output = String::new();

    output.push_str("pub enum ");
    output.push_str(&def.name);
    output.push_str(&build_generics(&def.generics, index));

    if let Some(where_clause) = build_where_clause(&def.generics.where_predicates, index) {
        output.push('\n');
        output.push_str(&where_clause);
        output.push('\n');
    }

    output.push_str(" {\n");

    if let Some(variants) = &def.variants {
        for variant in variants {
            // Add variant documentation
            if let Some(variant_docs) = &variant.docs {
                for line in variant_docs.lines() {
                    output.push_str(&format!("    /// {}\n", line));
                }
            }

            output.push_str("    ");
            output.push_str(&variant.name);

            // Format variant fields
            if let Some(tuple_fields) = &variant.tuple_fields {
                if !tuple_fields.is_empty() {
                    output.push('(');
                    output.push_str(&tuple_fields.join(", "));
                    output.push(')');
                }
            } else if let Some(struct_fields) = &variant.struct_fields {
                if !struct_fields.is_empty() {
                    output.push_str(" {\n");
                    for field in struct_fields {
                        if let Some(field_docs) = &field.docs {
                            for line in field_docs.lines() {
                                output.push_str(&format!("        /// {}\n", line));
                            }
                        }
                        output.push_str(&format!(
                            "        {} {}: {},\n",
                            field.visibility, field.name, field.type_name
                        ));
                    }
                    output.push_str("    }");
                } else {
                    output.push_str(" {}");
                }
            }

            output.push_str(",\n");
        }
    }

    output.push_str("}\n");

    Ok(output)
}

fn build_union_definition(def: &TypeInfo, index: &CrateIndex) -> Result<String> {
    let mut output = String::new();

    output.push_str("pub union ");
    output.push_str(&def.name);
    output.push_str(&build_generics(&def.generics, index));

    if let Some(where_clause) = build_where_clause(&def.generics.where_predicates, index) {
        output.push('\n');
        output.push_str(&where_clause);
        output.push('\n');
    }

    output.push_str(" {\n");

    if let Some(fields) = &def.fields {
        for field in fields {
            if let Some(field_docs) = &field.docs {
                for line in field_docs.lines() {
                    output.push_str(&format!("    /// {}\n", line));
                }
            }

            output.push_str(&format!(
                "    {} {}: {},\n",
                field.visibility, field.name, field.type_name
            ));
        }
    }

    output.push_str("}\n");

    Ok(output)
}

fn build_generics(generics: &Generics, index: &CrateIndex) -> String {
    if generics.params.is_empty() {
        return String::new();
    }

    let params: Vec<String> = generics
        .params
        .iter()
        .map(|param| build_generic_param(param, index))
        .collect();

    format!("<{}>", params.join(", "))
}

fn build_generic_param(param: &GenericParamDef, index: &CrateIndex) -> String {
    match &param.kind {
        GenericParamDefKind::Lifetime { outlives } => {
            let mut output = param.name.clone();
            if !outlives.is_empty() {
                output.push_str(": ");
                output.push_str(&outlives.join(" + "));
            }
            output
        }
        GenericParamDefKind::Type {
            bounds, default, ..
        } => {
            let mut output = param.name.clone();

            if !bounds.is_empty() {
                output.push_str(": ");
                let bound_strs: Vec<String> = bounds
                    .iter()
                    .map(|bound| build_generic_bound(bound, index))
                    .collect();
                output.push_str(&bound_strs.join(" + "));
            }

            if let Some(default_ty) = default {
                output.push_str(" = ");
                output.push_str(&index.format_type_for_syntax(default_ty));
            }

            output
        }
        GenericParamDefKind::Const { type_, default } => {
            let mut output = format!(
                "const {}: {}",
                param.name,
                index.format_type_for_syntax(type_)
            );

            if let Some(default_val) = default {
                output.push_str(" = ");
                output.push_str(default_val);
            }

            output
        }
    }
}

fn build_generic_bound(bound: &GenericBound, index: &CrateIndex) -> String {
    match bound {
        GenericBound::TraitBound {
            trait_,
            modifier,
            generic_params,
        } => {
            let mut output = String::new();

            // Handle modifier (?, for Sized)
            match modifier {
                rustdoc_types::TraitBoundModifier::Maybe => output.push('?'),
                rustdoc_types::TraitBoundModifier::MaybeConst => output.push_str("~const "),
                _ => {}
            }

            // Format trait name
            if let Some(summary) = index.paths().get(&trait_.id) {
                output.push_str(&summary.path.join("::"));
            } else {
                output.push_str("<trait>");
            }

            // TODO: Handle generic_params if needed
            if !generic_params.is_empty() {
                output.push_str("<...>");
            }

            output
        }
        GenericBound::Outlives(lifetime) => lifetime.clone(),
        GenericBound::Use(_) => "use<...>".to_string(),
    }
}

fn build_where_clause(predicates: &[WherePredicate], index: &CrateIndex) -> Option<String> {
    if predicates.is_empty() {
        return None;
    }

    let mut output = String::from("where");

    for (idx, predicate) in predicates.iter().enumerate() {
        if idx > 0 {
            output.push(',');
        }
        output.push('\n');
        output.push_str("    ");
        output.push_str(&build_where_predicate(predicate, index));
    }

    Some(output)
}

fn build_where_predicate(predicate: &WherePredicate, index: &CrateIndex) -> String {
    match predicate {
        WherePredicate::BoundPredicate {
            type_,
            bounds,
            generic_params,
        } => {
            let mut output = String::new();

            // Handle higher-ranked trait bounds (for<'a>)
            if !generic_params.is_empty() {
                output.push_str("for<");
                let params: Vec<String> = generic_params
                    .iter()
                    .map(|p| build_generic_param(p, index))
                    .collect();
                output.push_str(&params.join(", "));
                output.push_str("> ");
            }

            output.push_str(&index.format_type_for_syntax(type_));
            output.push_str(": ");

            let bound_strs: Vec<String> = bounds
                .iter()
                .map(|bound| build_generic_bound(bound, index))
                .collect();
            output.push_str(&bound_strs.join(" + "));

            output
        }
        WherePredicate::LifetimePredicate { lifetime, outlives } => {
            let mut output = lifetime.clone();
            if !outlives.is_empty() {
                output.push_str(": ");
                output.push_str(&outlives.join(" + "));
            }
            output
        }
        WherePredicate::EqPredicate { lhs, rhs } => {
            let rhs_str = match rhs {
                rustdoc_types::Term::Type(ty) => index.format_type_for_syntax(ty),
                rustdoc_types::Term::Constant(c) => format!("{{{}}}", c.expr),
            };
            format!("{} = {}", index.format_type_for_syntax(lhs), rhs_str)
        }
    }
}

/// Extract an ID from a rustdoc Type.
pub fn extract_id_from_type(ty: &Type) -> Option<&Id> {
    match ty {
        Type::ResolvedPath(path) => Some(&path.id),
        _ => None,
    }
}

/// Format a simple generic parameter name (without bounds or defaults).
pub fn format_generic_param_simple(param: &GenericParamDef) -> String {
    param.name.clone()
}

/// Extension trait providing type formatting capabilities for `CrateIndex`.
pub trait TypeFormatter {
    /// Formats a type for display (may use placeholder symbols).
    fn format_type(&self, ty: &Type) -> String;

    /// Formats a type for use in generated Rust syntax (uses valid placeholders).
    fn format_type_for_syntax(&self, ty: &Type) -> String;

    /// Formats a function's signature including generics and parameters.
    fn format_function_signature(&self, item: &Item) -> Option<String>;
}

impl TypeFormatter for CrateIndex {
    fn format_type(&self, ty: &Type) -> String {
        format_type_impl(self, ty, false)
    }

    fn format_type_for_syntax(&self, ty: &Type) -> String {
        format_type_impl(self, ty, true)
    }

    fn format_function_signature(&self, item: &Item) -> Option<String> {
        if let ItemEnum::Function(func) = &item.inner {
            let name = item.name.as_deref().unwrap_or("<unnamed>");
            let mut sig = format!("fn {}", name);

            if !func.generics.params.is_empty() {
                sig.push('<');
                let generic_names: Vec<String> = func
                    .generics
                    .params
                    .iter()
                    .map(format_generic_param_simple)
                    .collect();
                sig.push_str(&generic_names.join(", "));
                sig.push('>');
            }

            sig.push('(');
            let params: Vec<String> = func
                .sig
                .inputs
                .iter()
                .map(|(name, ty)| format!("{}: {}", name, self.format_type(ty)))
                .collect();
            sig.push_str(&params.join(", "));
            sig.push(')');

            if let Some(output) = &func.sig.output {
                sig.push_str(&format!(" -> {}", self.format_type(output)));
            }

            Some(sig)
        } else {
            None
        }
    }
}

/// Internal type formatter with mode selection.
/// When `for_syntax` is true, uses valid Rust identifiers instead of symbols.
fn format_type_impl(index: &CrateIndex, ty: &Type, for_syntax: bool) -> String {
    match ty {
        Type::ResolvedPath(path) => {
            if let Some(summary) = index.paths().get(&path.id) {
                let name = summary.path.last().unwrap_or(&"?".to_string()).clone();
                if let Some(args) = &path.args {
                    match args.as_ref() {
                        rustdoc_types::GenericArgs::AngleBracketed { args, constraints } => {
                            if args.is_empty() && constraints.is_empty() {
                                name
                            } else {
                                let arg_strs: Vec<String> = args
                                    .iter()
                                    .map(|arg| match arg {
                                        rustdoc_types::GenericArg::Lifetime(lt) => lt.clone(),
                                        rustdoc_types::GenericArg::Type(t) => {
                                            format_type_impl(index, t, for_syntax)
                                        }
                                        rustdoc_types::GenericArg::Const(c) => {
                                            format!("{{{}}}", c.expr)
                                        }
                                        rustdoc_types::GenericArg::Infer => "_".to_string(),
                                    })
                                    .collect();

                                if arg_strs.is_empty() {
                                    if for_syntax {
                                        format!("{}<_>", name)
                                    } else {
                                        format!("{}<...>", name)
                                    }
                                } else {
                                    format!("{}<{}>", name, arg_strs.join(", "))
                                }
                            }
                        }
                        rustdoc_types::GenericArgs::Parenthesized { inputs, output } => {
                            let input_strs: Vec<String> = inputs
                                .iter()
                                .map(|t| format_type_impl(index, t, for_syntax))
                                .collect();
                            let mut result = format!("{}({})", name, input_strs.join(", "));
                            if let Some(out) = output {
                                result.push_str(" -> ");
                                result.push_str(&format_type_impl(index, out, for_syntax));
                            }
                            result
                        }
                        rustdoc_types::GenericArgs::ReturnTypeNotation => name,
                    }
                } else {
                    name
                }
            } else if for_syntax {
                "()".to_string()
            } else {
                "<type>".to_string()
            }
        }
        Type::Generic(name) => name.clone(),
        Type::Primitive(name) => name.clone(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let mut s = String::from("&");
            if let Some(lt) = lifetime {
                s.push_str(lt);
                s.push(' ');
            }
            if *is_mutable {
                s.push_str("mut ");
            }
            s.push_str(&format_type_impl(index, type_, for_syntax));
            s
        }
        Type::Tuple(types) => {
            if types.is_empty() {
                "()".to_string()
            } else {
                let formatted: Vec<_> = types
                    .iter()
                    .map(|t| format_type_impl(index, t, for_syntax))
                    .collect();
                format!("({})", formatted.join(", "))
            }
        }
        Type::Slice(inner) => format!("[{}]", format_type_impl(index, inner, for_syntax)),
        Type::Array { type_, len } => {
            format!("[{}; {}]", format_type_impl(index, type_, for_syntax), len)
        }
        Type::RawPointer { is_mutable, type_ } => {
            if *is_mutable {
                format!("*mut {}", format_type_impl(index, type_, for_syntax))
            } else {
                format!("*const {}", format_type_impl(index, type_, for_syntax))
            }
        }
        Type::FunctionPointer(_) => {
            if for_syntax {
                "fn()".to_string()
            } else {
                "fn(...)".to_string()
            }
        }
        Type::QualifiedPath { .. } => {
            if for_syntax {
                "()".to_string()
            } else {
                "<qualified path>".to_string()
            }
        }
        _ => {
            if for_syntax {
                "()".to_string()
            } else {
                "<type>".to_string()
            }
        }
    }
}
