use rustdoc_types::{Crate, Id, Item, ItemEnum, Type};
use serde_json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::types::*;

pub struct DocIndex {
    krate: Crate,
    index: HashMap<Id, Item>,
    external_crates: HashMap<u32, String>,
}

impl DocIndex {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let krate: Crate = serde_json::from_str(&content)?;

        let index = krate.index.clone();

        let external_crates = krate
            .external_crates
            .iter()
            .map(|(id, crate_info)| (*id, crate_info.name.clone()))
            .collect();

        Ok(DocIndex {
            krate,
            index,
            external_crates,
        })
    }

    pub fn crate_info(&self) -> (Option<&str>, Option<&str>) {
        let name = self.root_module().and_then(|m| m.name.as_deref());
        (name, self.krate.crate_version.as_deref())
    }

    pub fn get_item(&self, id: &Id) -> Option<&Item> {
        self.index.get(id)
    }

    pub fn root_module(&self) -> Option<&Item> {
        self.index.get(&self.krate.root)
    }

    pub fn find_by_kind(&self, kind: ItemKind) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| matches_kind(&item.inner, kind))
            .collect()
    }

    pub fn search_all(&self, query: &str) -> Vec<SearchResult> {
        self.search_with_filter(query, None)
    }

    pub fn search_with_filter(
        &self,
        query: &str,
        filter_kind: Option<ItemKind>,
    ) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for item in self.index.values() {
            if let Some(kind_filter) = filter_kind
                && !matches_kind(&item.inner, kind_filter) {
                    continue;
                }

            if let Some(name) = &item.name {
                let name_lower = name.to_lowercase();
                if let Some(relevance) = calculate_relevance(&name_lower, &query_lower) {
                    let path = self.get_item_path_from_index(item);
                    results.push(SearchResult {
                        name: name.clone(),
                        path,
                        kind: item_kind_str(&item.inner).to_string(),
                        crate_name: None,
                        docs: item.docs.clone(),
                        id: Some(item.id),
                        relevance,
                        source_crate: None,
                    });
                }
            }
        }

        for (id, summary) in &self.krate.paths {
            if let Some(kind_filter) = filter_kind {
                let matches = match (kind_filter, &summary.kind) {
                    (ItemKind::Module, rustdoc_types::ItemKind::Module) => true,
                    (ItemKind::Struct, rustdoc_types::ItemKind::Struct) => true,
                    (ItemKind::Enum, rustdoc_types::ItemKind::Enum) => true,
                    (ItemKind::Function, rustdoc_types::ItemKind::Function) => true,
                    (ItemKind::Trait, rustdoc_types::ItemKind::Trait) => true,
                    (ItemKind::TypeAlias, rustdoc_types::ItemKind::TypeAlias) => true,
                    (ItemKind::Constant, rustdoc_types::ItemKind::Constant) => true,
                    (ItemKind::Static, rustdoc_types::ItemKind::Static) => true,
                    _ => false,
                };
                if !matches {
                    continue;
                }
            }

            if let Some(last_segment) = summary.path.last() {
                let name_lower = last_segment.to_lowercase();
                if let Some(relevance) = calculate_relevance(&name_lower, &query_lower) {
                    let crate_name = self.external_crates.get(&summary.crate_id).cloned();
                    let path = summary.path.join("::");
                    results.push(SearchResult {
                        name: last_segment.clone(),
                        path,
                        kind: format!("{:?}", summary.kind).to_lowercase(),
                        crate_name,
                        docs: None,
                        id: Some(*id),
                        relevance,
                        source_crate: None,
                    });
                }
            }
        }

        results.sort_by(|a, b| {
            b.relevance
                .cmp(&a.relevance)
                .then_with(|| a.name.cmp(&b.name))
        });

        results
    }

    pub fn find_public_path(&self, type_name: &str) -> Vec<String> {
        let mut paths = Vec::new();

        for summary in self.krate.paths.values() {
            if summary.path.last().map(|s| s.as_str()) == Some(type_name) {
                paths.push(summary.path.join("::"));
            }
        }

        paths.sort_by(|a, b| {
            let a_score = path_canonicality_score(a);
            let b_score = path_canonicality_score(b);
            b_score.cmp(&a_score)
        });

        paths
    }

    pub fn get_impls(&self, type_id: &Id) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| {
                if let ItemEnum::Impl(impl_item) = &item.inner {
                    extract_id_from_type(&impl_item.for_)
                        .map(|id| id == type_id)
                        .unwrap_or(false)
                } else {
                    false
                }
            })
            .collect()
    }

    pub fn find_trait_impls(&self, type_name: &str) -> Vec<TraitImplInfo> {
        let mut impls = Vec::new();

        for item in self.index.values() {
            if let ItemEnum::Impl(impl_item) = &item.inner {
                let for_type_matches = extract_id_from_type(&impl_item.for_)
                    .and_then(|id| self.get_item(id))
                    .and_then(|item| item.name.as_ref())
                    .map(|name| name.contains(type_name))
                    .unwrap_or(false);

                if for_type_matches {
                    let trait_name = impl_item
                        .trait_
                        .as_ref()
                        .map(|path| &path.id)
                        .and_then(|id| self.krate.paths.get(id))
                        .map(|summary| summary.path.join("::"));

                    impls.push(TraitImplInfo {
                        trait_name,
                        methods: impl_item.items.clone(),
                    });
                }
            }
        }

        impls
    }

    pub fn get_docs(&self, id: &Id) -> Option<&str> {
        self.get_item(id)?.docs.as_deref()
    }

    pub fn public_functions(&self) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| {
                matches!(item.inner, ItemEnum::Function(_))
                    && matches!(item.visibility, rustdoc_types::Visibility::Public)
            })
            .collect()
    }

    pub fn public_types(&self) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| {
                matches!(
                    item.inner,
                    ItemEnum::Struct(_) | ItemEnum::Enum(_) | ItemEnum::TypeAlias(_)
                ) && matches!(item.visibility, rustdoc_types::Visibility::Public)
            })
            .collect()
    }

    pub fn public_traits(&self) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| {
                matches!(item.inner, ItemEnum::Trait(_))
                    && matches!(item.visibility, rustdoc_types::Visibility::Public)
            })
            .collect()
    }

    pub fn format_function_signature(&self, item: &Item) -> Option<String> {
        if let ItemEnum::Function(func) = &item.inner {
            let name = item.name.as_deref().unwrap_or("<unnamed>");
            let mut sig = format!("fn {}", name);

            if !func.generics.params.is_empty() {
                sig.push('<');
                let generic_names: Vec<String> = func
                    .generics
                    .params
                    .iter()
                    .map(format_generic_param)
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

    pub fn format_type(&self, ty: &Type) -> String {
        self.format_type_impl(ty, false)
    }

    /// Format a type for use in generated Rust syntax (uses valid placeholders)
    pub fn format_type_for_syntax(&self, ty: &Type) -> String {
        self.format_type_impl(ty, true)
    }

    fn format_type_impl(&self, ty: &Type, for_syntax: bool) -> String {
        match ty {
            Type::ResolvedPath(path) => {
                if let Some(summary) = self.krate.paths.get(&path.id) {
                    let name = summary.path.last().unwrap_or(&"?".to_string()).clone();
                    if let Some(args) = &path.args {
                        // Try to format generic arguments
                        match args.as_ref() {
                            rustdoc_types::GenericArgs::AngleBracketed { args, constraints } => {
                                if args.is_empty() && constraints.is_empty() {
                                    name
                                } else {
                                    let arg_strs: Vec<String> = args.iter().map(|arg| {
                                        match arg {
                                            rustdoc_types::GenericArg::Lifetime(lt) => lt.clone(),
                                            rustdoc_types::GenericArg::Type(t) => self.format_type_impl(t, for_syntax),
                                            rustdoc_types::GenericArg::Const(c) => format!("{{{}}}", c.expr),
                                            rustdoc_types::GenericArg::Infer => "_".to_string(),
                                        }
                                    }).collect();

                                    if arg_strs.is_empty() {
                                        // If we have constraints but no args, show a placeholder
                                        if for_syntax {
                                            format!("{}<_>", name)  // Valid syntax placeholder
                                        } else {
                                            format!("{}<...>", name)  // Readable placeholder
                                        }
                                    } else {
                                        format!("{}<{}>", name, arg_strs.join(", "))
                                    }
                                }
                            }
                            rustdoc_types::GenericArgs::Parenthesized { inputs, output } => {
                                let input_strs: Vec<String> = inputs.iter().map(|t| self.format_type_impl(t, for_syntax)).collect();
                                let mut result = format!("{}({})", name, input_strs.join(", "));
                                if let Some(out) = output {
                                    result.push_str(" -> ");
                                    result.push_str(&self.format_type_impl(out, for_syntax));
                                }
                                result
                            }
                            rustdoc_types::GenericArgs::ReturnTypeNotation => {
                                // Return type notation (..): Not commonly used, treat as name only
                                name
                            }
                        }
                    } else {
                        name
                    }
                } else {
                    if for_syntax {
                        "()".to_string()  // Valid unit type placeholder
                    } else {
                        "<type>".to_string()
                    }
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
                s.push_str(&self.format_type_impl(type_, for_syntax));
                s
            }
            Type::Tuple(types) => {
                if types.is_empty() {
                    "()".to_string()
                } else {
                    let formatted: Vec<_> = types.iter().map(|t| self.format_type_impl(t, for_syntax)).collect();
                    format!("({})", formatted.join(", "))
                }
            }
            Type::Slice(inner) => format!("[{}]", self.format_type_impl(inner, for_syntax)),
            Type::Array { type_, len } => format!("[{}; {}]", self.format_type_impl(type_, for_syntax), len),
            Type::RawPointer { is_mutable, type_ } => {
                if *is_mutable {
                    format!("*mut {}", self.format_type_impl(type_, for_syntax))
                } else {
                    format!("*const {}", self.format_type_impl(type_, for_syntax))
                }
            }
            Type::FunctionPointer(_) => {
                if for_syntax {
                    "fn()".to_string()  // Valid function pointer placeholder
                } else {
                    "fn(...)".to_string()
                }
            }
            Type::QualifiedPath { .. } => {
                if for_syntax {
                    "()".to_string()  // Valid placeholder
                } else {
                    "<qualified path>".to_string()
                }
            }
            _ => {
                if for_syntax {
                    "()".to_string()  // Valid placeholder
                } else {
                    "<type>".to_string()
                }
            }
        }
    }

    pub fn format_item(&self, item: &Item) -> String {
        let kind = item_kind_str(&item.inner);
        let name = item.name.as_deref().unwrap_or("<unnamed>");
        let docs = item.docs.as_deref().unwrap_or("<no documentation>");

        let mut output = format!("{} {}\n", kind, name);

        if let Some(sig) = self.format_function_signature(item) {
            output.push_str(&format!("{}\n", sig));
        }

        output.push_str(
            &docs
                .lines()
                .take(3)
                .map(|l| format!("  {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        output.push('\n');

        output
    }

    fn get_item_path_from_index(&self, item: &Item) -> String {
        if let Some(summary) = self.krate.paths.get(&item.id) {
            return summary.path.join("::");
        }

        if let Some(name) = &item.name {
            name.clone()
        } else {
            "<unnamed>".to_string()
        }
    }

    pub fn get_item_path(&self, item: &Item) -> String {
        self.get_item_path_from_index(item)
    }

    pub fn krate(&self) -> &Crate {
        &self.krate
    }
}
