//! Type formatting utilities for documentation display.
//!
//! This module provides type formatting capabilities via the `TypeFormatter` trait,
//! converting rustdoc type information into human-readable strings.

use crate::search::rustdoc::CrateIndex;
use rustdoc_types::{GenericArgs, GenericParamDef, Id, Item, ItemEnum, Path, Type};

/// Extract an ID from a rustdoc Type.
pub(crate) fn extract_id_from_type(ty: &Type) -> Option<&Id> {
    match ty {
        Type::ResolvedPath(path) => Some(&path.id),
        _ => None,
    }
}

/// Format a simple generic parameter name (without bounds or defaults).
pub(crate) fn format_generic_param_simple(param: &GenericParamDef) -> String {
    param.name.clone()
}

/// Extension trait providing type formatting capabilities for `CrateIndex`.
pub trait TypeFormatter {
    /// Formats a type for display (may use placeholder symbols).
    fn format_type(&self, ty: &Type) -> String;

    /// Formats a function's signature including generics and parameters.
    fn format_function_signature(&self, item: &Item) -> Option<String>;
}

impl TypeFormatter for CrateIndex {
    fn format_type(&self, ty: &Type) -> String {
        format_type_impl(self, ty)
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

/// Internal type formatter implementation.
fn format_type_impl(index: &CrateIndex, ty: &Type) -> String {
    match ty {
        Type::ResolvedPath(path) => format_resolved_path(index, path),
        Type::Generic(name) | Type::Primitive(name) => name.clone(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => format_borrowed_ref(index, lifetime.as_deref(), *is_mutable, type_),
        Type::Tuple(types) if types.is_empty() => "()".to_string(),
        Type::Tuple(types) => {
            let formatted: Vec<_> = types.iter().map(|t| format_type_impl(index, t)).collect();
            format!("({})", formatted.join(", "))
        }
        Type::Slice(inner) => format!("[{}]", format_type_impl(index, inner)),
        Type::Array { type_, len } => {
            format!("[{}; {}]", format_type_impl(index, type_), len)
        }
        Type::RawPointer { is_mutable, type_ } => {
            let prefix = if *is_mutable { "*mut " } else { "*const " };
            format!("{}{}", prefix, format_type_impl(index, type_))
        }
        Type::FunctionPointer(_) => "fn(...)".to_string(),
        Type::QualifiedPath { .. } => "<qualified path>".to_string(),
        _ => "<type>".to_string(),
    }
}

/// Formats a resolved path type (e.g., `Vec<T>`, `HashMap<K, V>`).
fn format_resolved_path(index: &CrateIndex, path: &Path) -> String {
    let Some(summary) = index.paths().get(&path.id) else {
        return "<type>".to_string();
    };

    let name = summary.path.last().map(String::as_str).unwrap_or("?");
    let Some(args) = &path.args else {
        return name.to_string();
    };

    format_generic_args(index, name, args.as_ref())
}

/// Formats generic arguments (angle-bracketed or parenthesized).
fn format_generic_args(index: &CrateIndex, name: &str, args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            if args.is_empty() && constraints.is_empty() {
                return name.to_string();
            }

            let arg_strs: Vec<String> = args.iter().map(|arg| format_arg(index, arg)).collect();

            if arg_strs.is_empty() {
                format!("{}<...>", name)
            } else {
                format!("{}<{}>", name, arg_strs.join(", "))
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let input_strs: Vec<String> =
                inputs.iter().map(|t| format_type_impl(index, t)).collect();
            let mut result = format!("{}({})", name, input_strs.join(", "));
            if let Some(out) = output {
                result.push_str(" -> ");
                result.push_str(&format_type_impl(index, out));
            }
            result
        }
        GenericArgs::ReturnTypeNotation => name.to_string(),
    }
}

/// Formats a single generic argument.
fn format_arg(index: &CrateIndex, arg: &rustdoc_types::GenericArg) -> String {
    match arg {
        rustdoc_types::GenericArg::Lifetime(lt) => lt.clone(),
        rustdoc_types::GenericArg::Type(t) => format_type_impl(index, t),
        rustdoc_types::GenericArg::Const(c) => format!("{{{}}}", c.expr),
        rustdoc_types::GenericArg::Infer => "_".to_string(),
    }
}

/// Formats a borrowed reference type.
fn format_borrowed_ref(
    index: &CrateIndex,
    lifetime: Option<&str>,
    is_mutable: bool,
    inner: &Type,
) -> String {
    let mut s = String::with_capacity(32);
    s.push('&');
    if let Some(lt) = lifetime {
        s.push_str(lt);
        s.push(' ');
    }
    if is_mutable {
        s.push_str("mut ");
    }
    s.push_str(&format_type_impl(index, inner));
    s
}
