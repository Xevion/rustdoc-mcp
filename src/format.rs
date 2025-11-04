use crate::doc::DocIndex;
use crate::handlers::get_type_definition::TypeDefinition;
use rustdoc_types::{GenericBound, GenericParamDef, GenericParamDefKind, Generics, WherePredicate};

/// Generate unformatted Rust syntax for a type definition
pub fn generate_rust_syntax(
    def: &TypeDefinition,
    doc: &DocIndex,
) -> Result<String, Box<dyn std::error::Error>> {
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
    match def.kind.as_str() {
        "struct" => {
            output.push_str(&format_struct_definition(def, doc)?);
        }
        "enum" => {
            output.push_str(&format_enum_definition(def, doc)?);
        }
        "union" => {
            output.push_str(&format_union_definition(def, doc)?);
        }
        _ => {}
    }

    Ok(output)
}

/// Format a type definition as valid Rust syntax
pub fn format_type_as_rust(
    def: &TypeDefinition,
    doc: &DocIndex,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = generate_rust_syntax(def, doc)?;

    // Parse and format with prettyplease
    let syntax_tree = syn::parse_file(&output)?;
    let formatted = prettyplease::unparse(&syntax_tree);

    Ok(formatted)
}

fn format_struct_definition(
    def: &TypeDefinition,
    doc: &DocIndex,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();

    output.push_str("pub struct ");
    output.push_str(&def.name);
    output.push_str(&format_generics(&def.generics, doc));

    if let Some(where_clause) = format_where_clause(&def.generics.where_predicates, doc) {
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

fn format_enum_definition(
    def: &TypeDefinition,
    doc: &DocIndex,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();

    output.push_str("pub enum ");
    output.push_str(&def.name);
    output.push_str(&format_generics(&def.generics, doc));

    if let Some(where_clause) = format_where_clause(&def.generics.where_predicates, doc) {
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

fn format_union_definition(
    def: &TypeDefinition,
    doc: &DocIndex,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = String::new();

    output.push_str("pub union ");
    output.push_str(&def.name);
    output.push_str(&format_generics(&def.generics, doc));

    if let Some(where_clause) = format_where_clause(&def.generics.where_predicates, doc) {
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

fn format_generics(generics: &Generics, doc: &DocIndex) -> String {
    if generics.params.is_empty() {
        return String::new();
    }

    let params: Vec<String> = generics
        .params
        .iter()
        .map(|param| format_generic_param(param, doc))
        .collect();

    format!("<{}>", params.join(", "))
}

fn format_generic_param(param: &GenericParamDef, doc: &DocIndex) -> String {
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
                    .map(|bound| format_generic_bound(bound, doc))
                    .collect();
                output.push_str(&bound_strs.join(" + "));
            }

            if let Some(default_ty) = default {
                output.push_str(" = ");
                output.push_str(&doc.format_type_for_syntax(default_ty));
            }

            output
        }
        GenericParamDefKind::Const { type_, default } => {
            let mut output = format!(
                "const {}: {}",
                param.name,
                doc.format_type_for_syntax(type_)
            );

            if let Some(default_val) = default {
                output.push_str(" = ");
                output.push_str(default_val);
            }

            output
        }
    }
}

fn format_generic_bound(bound: &GenericBound, doc: &DocIndex) -> String {
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
            if let Some(summary) = doc.krate().paths.get(&trait_.id) {
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

fn format_where_clause(predicates: &[WherePredicate], doc: &DocIndex) -> Option<String> {
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
        output.push_str(&format_where_predicate(predicate, doc));
    }

    Some(output)
}

fn format_where_predicate(predicate: &WherePredicate, doc: &DocIndex) -> String {
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
                    .map(|p| format_generic_param(p, doc))
                    .collect();
                output.push_str(&params.join(", "));
                output.push_str("> ");
            }

            output.push_str(&doc.format_type_for_syntax(type_));
            output.push_str(": ");

            let bound_strs: Vec<String> = bounds
                .iter()
                .map(|bound| format_generic_bound(bound, doc))
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
                rustdoc_types::Term::Type(ty) => doc.format_type_for_syntax(ty),
                rustdoc_types::Term::Constant(c) => format!("{{{}}}", c.expr),
            };
            format!("{} = {}", doc.format_type_for_syntax(lhs), rhs_str)
        }
    }
}
