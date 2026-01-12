//! Type formatting utilities for documentation display.
//!
//! This module provides type formatting capabilities via the `TypeFormatter` struct,
//! converting rustdoc type information into human-readable strings.

use crate::search::rustdoc::CrateIndex;
use rustdoc_types::{
    AssocItemConstraintKind, GenericArg, GenericArgs, GenericBound, GenericParamDef,
    GenericParamDefKind, Generics, Item, ItemEnum, Path, Term, TraitBoundModifier, Type,
    WherePredicate,
};
use std::fmt::{self, Write};

/// Type formatter providing formatting capabilities for rustdoc types.
///
/// Holds a reference to a `CrateIndex` for resolving paths and type information.
/// Provides both `write_*` methods (efficient, write to any buffer) and `format_*`
/// methods (convenient, return String).
pub struct TypeFormatter<'a> {
    index: &'a CrateIndex,
}

impl<'a> TypeFormatter<'a> {
    /// Create a new formatter for the given crate index.
    pub fn new(index: &'a CrateIndex) -> Self {
        Self { index }
    }

    /// Get the underlying crate index.
    pub fn index(&self) -> &'a CrateIndex {
        self.index
    }

    /// Write a type to the output buffer.
    pub fn write_type<W: Write>(&self, w: &mut W, root: &Type) -> fmt::Result {
        match root {
            Type::ResolvedPath(path) => self.write_resolved_path(w, path),
            Type::Generic(name) | Type::Primitive(name) => w.write_str(name),
            Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } => self.write_borrowed_ref(w, lifetime.as_deref(), *is_mutable, type_),
            Type::Tuple(types) if types.is_empty() => w.write_str("()"),
            Type::Tuple(types) => {
                w.write_char('(')?;
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        w.write_str(", ")?;
                    }
                    self.write_type(w, t)?;
                }
                w.write_char(')')
            }
            Type::Slice(inner) => {
                w.write_char('[')?;
                self.write_type(w, inner)?;
                w.write_char(']')
            }
            Type::Array {
                type_: inner_type,
                len,
            } => {
                w.write_char('[')?;
                self.write_type(w, inner_type)?;
                write!(w, "; {}]", len)
            }
            Type::RawPointer { is_mutable, type_ } => {
                w.write_str(if *is_mutable { "*mut " } else { "*const " })?;
                self.write_type(w, type_)
            }
            Type::FunctionPointer(_) => w.write_str("fn(...)"),
            Type::QualifiedPath { .. } => w.write_str("<qualified path>"),
            // TODO: Handle these properly
            Type::DynTrait(..) | Type::Pat { .. } | Type::ImplTrait(..) | Type::Infer => {
                w.write_str("<type>")
            }
        }
    }

    /// Write complete angle-bracketed generics: `<K: Eq + Hash, V, S = RandomState>`
    /// Writes nothing if no non-synthetic params.
    pub fn write_generics<W: Write>(&self, w: &mut W, generics: &Generics) -> fmt::Result {
        let real_params: Vec<_> = generics
            .params
            .iter()
            .filter(|p| {
                !matches!(
                    &p.kind,
                    GenericParamDefKind::Type {
                        is_synthetic: true,
                        ..
                    }
                )
            })
            .collect();

        if real_params.is_empty() {
            return Ok(());
        }

        w.write_char('<')?;
        for (i, p) in real_params.iter().enumerate() {
            if i > 0 {
                w.write_str(", ")?;
            }
            self.write_generic_param_full(w, p)?;
        }
        w.write_char('>')
    }

    /// Write where clause with dynamic inline/multi-line based on complexity.
    /// Writes nothing if no predicates.
    pub fn write_where_clause<W: Write>(
        &self,
        w: &mut W,
        predicates: &[WherePredicate],
        current_line_len: usize,
    ) -> fmt::Result {
        if predicates.is_empty() {
            return Ok(());
        }

        let formatted: Vec<_> = predicates
            .iter()
            .map(|p| self.format_where_predicate(p))
            .collect();

        let has_hrtb = predicates.iter().any(|p| {
            matches!(
                p,
                WherePredicate::BoundPredicate { generic_params, .. } if !generic_params.is_empty()
            )
        });

        let inline = format!(" where {}", formatted.join(", "));
        let use_multiline =
            predicates.len() > 2 || current_line_len + inline.len() > 80 || has_hrtb;

        if use_multiline {
            w.write_str("\nwhere\n")?;
            for (i, pred) in formatted.iter().enumerate() {
                w.write_str("    ")?;
                w.write_str(pred)?;
                if i < formatted.len() - 1 {
                    w.write_char(',')?;
                }
                w.write_char('\n')?;
            }
            Ok(())
        } else {
            w.write_str(&inline)
        }
    }

    /// Write supertrait bounds for traits: `: Clone + Debug`
    /// Uses same threshold logic for inline vs where-style.
    pub fn write_supertrait_bounds<W: Write>(
        &self,
        w: &mut W,
        bounds: &[GenericBound],
        current_line_len: usize,
    ) -> fmt::Result {
        if bounds.is_empty() {
            return Ok(());
        }

        let formatted: Vec<_> = bounds
            .iter()
            .map(|b| self.format_generic_bound(b))
            .collect();

        let inline = format!(": {}", formatted.join(" + "));

        if bounds.len() > 2 || current_line_len + inline.len() > 80 {
            w.write_str("\nwhere\n    Self: ")?;
            w.write_str(&formatted.join(" + "))
        } else {
            w.write_str(&inline)
        }
    }

    /// Write a function signature including generics and parameters.
    /// Returns Ok(true) if signature was written, Ok(false) if item is not a function.
    pub fn write_function_signature<W: Write>(&self, w: &mut W, item: &Item) -> fmt::Result {
        let ItemEnum::Function(func) = &item.inner else {
            return Ok(());
        };

        let name = item.name.as_deref().unwrap_or("<unnamed>");
        w.write_str("fn ")?;
        w.write_str(name)?;

        self.write_generics(w, &func.generics)?;

        w.write_char('(')?;
        for (i, (param_name, ty)) in func.sig.inputs.iter().enumerate() {
            if i > 0 {
                w.write_str(", ")?;
            }
            write!(w, "{}: ", param_name)?;
            self.write_type(w, ty)?;
        }
        w.write_char(')')?;

        if let Some(output) = &func.sig.output {
            w.write_str(" -> ")?;
            self.write_type(w, output)?;
        }

        // Calculate current length for where clause threshold
        // This is approximate but good enough for the heuristic
        let sig_len = name.len() + 10; // rough estimate
        self.write_where_clause(w, &func.generics.where_predicates, sig_len)
    }

    /// Format generic args for a bound (no type name prefix).
    /// Example: `<T>`, `<Item = usize>`
    /// Private helper for internal use within format_generic_bound.
    fn format_bound_args(&self, args: &GenericArgs) -> String {
        let mut s = String::new();
        let _ = self.write_bound_args(&mut s, args);
        s
    }

    /// Write generic args for a type path.
    fn write_type_args<W: Write>(&self, w: &mut W, name: &str, args: &GenericArgs) -> fmt::Result {
        match args {
            GenericArgs::AngleBracketed { args, constraints } => {
                if args.is_empty() && constraints.is_empty() {
                    return w.write_str(name);
                }

                w.write_str(name)?;
                w.write_char('<')?;
                self.write_angle_args(w, args)?;
                w.write_char('>')
            }
            GenericArgs::Parenthesized { inputs, output } => {
                w.write_str(name)?;
                self.write_parenthesized_args(w, inputs, output.as_ref())
            }
            GenericArgs::ReturnTypeNotation => w.write_str(name),
        }
    }

    /// Write generic args for a bound context (with constraints).
    fn write_bound_args<W: Write>(&self, w: &mut W, args: &GenericArgs) -> fmt::Result {
        match args {
            GenericArgs::AngleBracketed { args, constraints } => {
                if args.is_empty() && constraints.is_empty() {
                    return Ok(());
                }

                w.write_char('<')?;
                self.write_angle_args(w, args)?;

                // Add constraints like Item = usize or Item: Clone
                for (i, constraint) in constraints.iter().enumerate() {
                    if !args.is_empty() || i > 0 {
                        w.write_str(", ")?;
                    }
                    self.write_constraint(w, constraint)?;
                }

                w.write_char('>')
            }
            GenericArgs::Parenthesized { inputs, output } => {
                self.write_parenthesized_args(w, inputs, output.as_ref())
            }
            GenericArgs::ReturnTypeNotation => w.write_str("(..)"),
        }
    }

    /// Check if a path is from std/core/alloc (use short name).
    fn is_std_path(path: &[String]) -> bool {
        matches!(
            path.first().map(String::as_str),
            Some("std" | "core" | "alloc")
        )
    }

    /// Write a resolved path type.
    fn write_resolved_path<W: Write>(&self, w: &mut W, path: &Path) -> fmt::Result {
        let Some(summary) = self.index.paths().get(&path.id) else {
            return w.write_str("<type>");
        };

        let name = summary.path.last().map(String::as_str).unwrap_or("?");
        match &path.args {
            Some(args) => self.write_type_args(w, name, args.as_ref()),
            None => w.write_str(name),
        }
    }

    /// Write a borrowed reference type.
    fn write_borrowed_ref<W: Write>(
        &self,
        w: &mut W,
        lifetime: Option<&str>,
        is_mutable: bool,
        inner: &Type,
    ) -> fmt::Result {
        w.write_char('&')?;
        if let Some(lt) = lifetime {
            w.write_str(lt)?;
            w.write_char(' ')?;
        }
        if is_mutable {
            w.write_str("mut ")?;
        }
        self.write_type(w, inner)
    }

    /// Write angle-bracketed args (shared between type and bound contexts).
    fn write_angle_args<W: Write>(&self, w: &mut W, args: &[GenericArg]) -> fmt::Result {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                w.write_str(", ")?;
            }
            self.write_generic_arg(w, arg)?;
        }
        Ok(())
    }

    /// Write parenthesized args (shared between type and bound contexts).
    fn write_parenthesized_args<W: Write>(
        &self,
        w: &mut W,
        inputs: &[Type],
        output: Option<&Type>,
    ) -> fmt::Result {
        w.write_char('(')?;
        for (i, t) in inputs.iter().enumerate() {
            if i > 0 {
                w.write_str(", ")?;
            }
            self.write_type(w, t)?;
        }
        w.write_char(')')?;
        if let Some(out) = output {
            w.write_str(" -> ")?;
            self.write_type(w, out)?;
        }
        Ok(())
    }

    /// Write a single generic argument.
    fn write_generic_arg<W: Write>(&self, w: &mut W, arg: &GenericArg) -> fmt::Result {
        match arg {
            GenericArg::Lifetime(lt) => w.write_str(lt),
            GenericArg::Type(t) => self.write_type(w, t),
            GenericArg::Const(c) => write!(w, "{{{}}}", c.expr),
            GenericArg::Infer => w.write_char('_'),
        }
    }

    /// Write an associated item constraint (for bounds).
    fn write_constraint<W: Write>(
        &self,
        w: &mut W,
        constraint: &rustdoc_types::AssocItemConstraint,
    ) -> fmt::Result {
        match &constraint.binding {
            AssocItemConstraintKind::Equality(term) => {
                write!(w, "{} = ", constraint.name)?;
                self.write_term(w, term)
            }
            AssocItemConstraintKind::Constraint(bounds) => {
                write!(w, "{}: ", constraint.name)?;
                for (i, b) in bounds.iter().enumerate() {
                    if i > 0 {
                        w.write_str(" + ")?;
                    }
                    w.write_str(&self.format_generic_bound(b))?;
                }
                Ok(())
            }
        }
    }

    /// Write a Term (Type or Constant).
    fn write_term<W: Write>(&self, w: &mut W, term: &Term) -> fmt::Result {
        match term {
            Term::Type(ty) => self.write_type(w, ty),
            Term::Constant(c) => w.write_str(&c.expr),
        }
    }

    /// Format a type for display. Private helper for internal string building.
    fn format_type(&self, ty: &Type) -> String {
        let mut s = String::new();
        let _ = self.write_type(&mut s, ty);
        s
    }

    /// Format a path for use in bounds - short for std, qualified for external.
    fn format_path_for_bound(&self, path: &Path) -> String {
        let Some(summary) = self.index.paths().get(&path.id) else {
            return "/* <path> */".to_string();
        };

        if Self::is_std_path(&summary.path) {
            summary
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| "/* <path> */".to_string())
        } else {
            match (summary.path.first(), summary.path.last()) {
                (Some(crate_name), Some(item_name)) if crate_name != item_name => {
                    format!("{}::{}", crate_name, item_name)
                }
                (_, Some(name)) => name.clone(),
                _ => "/* <path> */".to_string(),
            }
        }
    }

    /// Format a single generic bound.
    fn format_generic_bound(&self, bound: &GenericBound) -> String {
        match bound {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                let mut result = String::new();

                // HRTB: for<'a>
                if !generic_params.is_empty() {
                    result.push_str("for<");
                    let lifetimes: Vec<_> =
                        generic_params.iter().map(|p| p.name.as_str()).collect();
                    result.push_str(&lifetimes.join(", "));
                    result.push_str("> ");
                }

                // Modifier: ?, ~const
                match modifier {
                    TraitBoundModifier::Maybe => result.push('?'),
                    TraitBoundModifier::MaybeConst => result.push_str("~const "),
                    TraitBoundModifier::None => {}
                }

                // Trait path with generic args
                result.push_str(&self.format_path_for_bound(trait_));
                if let Some(args) = &trait_.args {
                    result.push_str(&self.format_bound_args(args));
                }

                result
            }
            GenericBound::Outlives(lifetime) => lifetime.clone(),
            GenericBound::Use(_) => "/* <use> */".to_string(),
        }
    }

    /// Write a complete generic parameter with bounds and defaults.
    fn write_generic_param_full<W: Write>(
        &self,
        w: &mut W,
        param: &GenericParamDef,
    ) -> fmt::Result {
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                w.write_str(&param.name)?;
                if !outlives.is_empty() {
                    write!(w, ": {}", outlives.join(" + "))?;
                }
                Ok(())
            }
            GenericParamDefKind::Type {
                bounds,
                default,
                is_synthetic: _,
            } => {
                w.write_str(&param.name)?;

                if !bounds.is_empty() {
                    w.write_str(": ")?;
                    for (i, b) in bounds.iter().enumerate() {
                        if i > 0 {
                            w.write_str(" + ")?;
                        }
                        w.write_str(&self.format_generic_bound(b))?;
                    }
                }

                if let Some(default_ty) = default {
                    w.write_str(" = ")?;
                    self.write_type(w, default_ty)?;
                }
                Ok(())
            }
            GenericParamDefKind::Const { type_, default } => {
                write!(w, "const {}: ", param.name)?;
                self.write_type(w, type_)?;
                if let Some(default_val) = default {
                    write!(w, " = {}", default_val)?;
                }
                Ok(())
            }
        }
    }

    /// Format a single where predicate.
    fn format_where_predicate(&self, pred: &WherePredicate) -> String {
        match pred {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                let mut result = String::new();

                // HRTB: for<'a>
                if !generic_params.is_empty() {
                    result.push_str("for<");
                    let params: Vec<_> = generic_params.iter().map(|p| p.name.as_str()).collect();
                    result.push_str(&params.join(", "));
                    result.push_str("> ");
                }

                result.push_str(&self.format_type(type_));
                result.push_str(": ");

                let bounds_str: Vec<_> = bounds
                    .iter()
                    .map(|b| self.format_generic_bound(b))
                    .collect();
                result.push_str(&bounds_str.join(" + "));

                result
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                format!("{}: {}", lifetime, outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                let mut s = self.format_type(lhs);
                s.push_str(" = ");
                let _ = self.write_term(&mut s, rhs);
                s
            }
        }
    }
}
