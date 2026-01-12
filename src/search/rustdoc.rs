//! Rustdoc JSON indexing and crate data structures.
//!
//! This module provides the `CrateIndex` structure for working with rustdoc JSON output,
//! along with utilities for item kind matching and conversion.

use crate::error::Result;
use anyhow::Context;
use rmcp::schemars;
use rustdoc_types::{
    Crate, Id, Item, ItemEnum, ItemKind as RustdocItemKind, ItemSummary, MacroKind, ProcMacro,
};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::path::Path;

/// Information about a trait implementation.
#[derive(Debug, Clone)]
pub struct TraitImplInfo {
    pub trait_name: Option<String>,
    pub methods: Vec<Id>,
}

/// DO NOT add doc comments to individual variants - this causes schemars to generate
/// `oneOf` schemas instead of simple `enum` arrays, breaking MCP client enum handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Module,
    Struct,
    Enum,
    Function,
    Trait,
    TypeAlias,
    Constant,
    Static,
}

/// Check if an ItemEnum matches a specific ItemKind.
pub(crate) fn matches_kind(inner: &ItemEnum, kind: ItemKind) -> bool {
    matches!(
        (inner, kind),
        (ItemEnum::Module(_), ItemKind::Module)
            | (ItemEnum::Struct(_), ItemKind::Struct)
            | (ItemEnum::Enum(_), ItemKind::Enum)
            | (ItemEnum::Function(_), ItemKind::Function)
            | (ItemEnum::Trait(_), ItemKind::Trait)
            | (ItemEnum::TypeAlias(_), ItemKind::TypeAlias)
            | (ItemEnum::Constant { .. }, ItemKind::Constant)
            | (ItemEnum::Static(_), ItemKind::Static)
    )
}

/// Convert an ItemEnum to its corresponding rustdoc ItemKind.
pub(crate) fn item_enum_to_kind(inner: &ItemEnum) -> RustdocItemKind {
    match inner {
        ItemEnum::Module(_) => RustdocItemKind::Module,
        ItemEnum::ExternCrate { .. } => RustdocItemKind::ExternCrate,
        ItemEnum::Use(_) => RustdocItemKind::Use,
        ItemEnum::Union(_) => RustdocItemKind::Union,
        ItemEnum::Struct(_) => RustdocItemKind::Struct,
        ItemEnum::StructField(_) => RustdocItemKind::StructField,
        ItemEnum::Enum(_) => RustdocItemKind::Enum,
        ItemEnum::Variant(_) => RustdocItemKind::Variant,
        ItemEnum::Function(_) => RustdocItemKind::Function,
        ItemEnum::Trait(_) => RustdocItemKind::Trait,
        ItemEnum::TraitAlias(_) => RustdocItemKind::TraitAlias,
        ItemEnum::Impl(_) => RustdocItemKind::Impl,
        ItemEnum::TypeAlias(_) => RustdocItemKind::TypeAlias,
        ItemEnum::Constant { .. } => RustdocItemKind::Constant,
        ItemEnum::Static(_) => RustdocItemKind::Static,
        ItemEnum::ExternType => RustdocItemKind::ExternType,
        ItemEnum::ProcMacro(ProcMacro {
            kind: MacroKind::Attr,
            ..
        }) => RustdocItemKind::ProcAttribute,
        ItemEnum::ProcMacro(ProcMacro {
            kind: MacroKind::Derive,
            ..
        }) => RustdocItemKind::ProcDerive,
        ItemEnum::Macro(_)
        | ItemEnum::ProcMacro(ProcMacro {
            kind: MacroKind::Bang,
            ..
        }) => RustdocItemKind::Macro,
        ItemEnum::Primitive(_) => RustdocItemKind::Primitive,
        ItemEnum::AssocConst { .. } => RustdocItemKind::AssocConst,
        ItemEnum::AssocType { .. } => RustdocItemKind::AssocType,
    }
}

/// Get a string representation of an item's kind.
pub(crate) fn item_kind_str(inner: &ItemEnum) -> &'static str {
    match inner {
        ItemEnum::Module(_) => "module",
        ItemEnum::Struct(_) => "struct",
        ItemEnum::Enum(_) => "enum",
        ItemEnum::Function(_) => "fn",
        ItemEnum::Trait(_) => "trait",
        ItemEnum::TypeAlias(_) => "type",
        ItemEnum::Constant { .. } => "const",
        ItemEnum::Static(_) => "static",
        ItemEnum::StructField(_) => "field",
        ItemEnum::Variant(_) => "variant",
        ItemEnum::Impl(_) => "impl",
        ItemEnum::Use(_) => "use",
        ItemEnum::Union(_) => "union",
        ItemEnum::Macro(_) => "macro",
        ItemEnum::ProcMacro(_) => "proc_macro",
        ItemEnum::Primitive(_) => "primitive",
        ItemEnum::AssocConst { .. } => "assoc_const",
        ItemEnum::AssocType { .. } => "assoc_type",
        _ => "item",
    }
}

pub struct CrateIndex {
    crate_data: Crate,
    pub index: HashMap<Id, Item>,
    _external_crates: HashMap<u32, String>,
}

impl CrateIndex {
    /// Loads rustdoc JSON output and builds an index of all items.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read rustdoc JSON at {}", path.display()))?;
        let crate_data: Crate =
            serde_json::from_str(&content).context("Failed to parse rustdoc JSON")?;

        let index = crate_data.index.clone();

        let external_crates = crate_data
            .external_crates
            .iter()
            .map(|(id, crate_info)| (*id, crate_info.name.clone()))
            .collect();

        Ok(CrateIndex {
            crate_data,
            index,
            _external_crates: external_crates,
        })
    }

    pub fn crate_info(&self) -> (Option<&str>, Option<&str>) {
        let name = self.root_module().and_then(|m| m.name.as_deref());
        (name, self.crate_data.crate_version.as_deref())
    }

    /// Get access to the underlying Crate structure
    pub fn crate_data(&self) -> &Crate {
        &self.crate_data
    }

    /// Get access to the crate's paths mapping
    pub fn paths(&self) -> &HashMap<Id, ItemSummary> {
        &self.crate_data.paths
    }

    pub fn get_item(&self, id: &Id) -> Option<&Item> {
        self.index.get(id)
    }

    pub fn root_module(&self) -> Option<&Item> {
        self.index.get(&self.crate_data.root)
    }

    /// Get the root item ID
    pub fn root(&self) -> &Id {
        &self.crate_data.root
    }

    /// Get the crate name
    pub fn name(&self) -> &str {
        self.root_module()
            .and_then(|m| m.name.as_deref())
            .unwrap_or("<unnamed>")
    }

    /// Get the path to an item by its ID
    pub fn path(&self, id: &Id) -> Option<crate::item::ItemPath<'_>> {
        self.crate_data.paths.get(id).map(|summary| summary.into())
    }

    /// Get an ItemRef for an item by ID
    pub fn get<'a>(
        &'a self,
        query: &'a crate::search::QueryContext,
        id: &Id,
    ) -> Option<crate::item::ItemRef<'a, Item>> {
        use crate::item::ItemRef;
        self.get_item(id)
            .map(|item| ItemRef::builder(query, self, item).build())
    }

    pub fn find_by_kind(&self, kind: ItemKind) -> Vec<&Item> {
        self.index
            .values()
            .filter(|item| matches_kind(&item.inner, kind))
            .collect()
    }

    /// Finds all public paths to items with the given name, sorted by canonicality.
    /// More canonical paths (shorter, fewer generics) appear first.
    pub fn find_public_path(&self, type_name: &str) -> Vec<String> {
        let mut paths = Vec::new();

        for summary in self.crate_data.paths.values() {
            if summary.path.last().map(|s| s.as_str()) == Some(type_name) {
                paths.push(summary.path.join("::"));
            }
        }

        paths.sort_by(|a, b| {
            use crate::search::path_canonicality_score;
            let a_score = path_canonicality_score(a);
            let b_score = path_canonicality_score(b);
            b_score.cmp(&a_score)
        });

        paths
    }

    /// Returns all impl blocks for the given type ID.
    pub fn get_impls(&self, type_id: &Id) -> Vec<&Item> {
        use rustdoc_types::Type;
        self.index
            .values()
            .filter(|item| {
                if let ItemEnum::Impl(impl_item) = &item.inner {
                    match &impl_item.for_ {
                        Type::ResolvedPath(path) => path.id == *type_id,
                        _ => false,
                    }
                } else {
                    false
                }
            })
            .collect()
    }

    /// Finds all trait implementations for types matching the given name.
    pub fn find_trait_impls(&self, type_name: &str) -> Vec<TraitImplInfo> {
        use rustdoc_types::Type;
        let mut impls = Vec::new();

        for item in self.index.values() {
            if let ItemEnum::Impl(impl_item) = &item.inner {
                let for_type_matches = match &impl_item.for_ {
                    Type::ResolvedPath(path) => self
                        .get_item(&path.id)
                        .and_then(|item| item.name.as_ref())
                        .map(|name| name.contains(type_name))
                        .unwrap_or(false),
                    _ => false,
                };

                if for_type_matches {
                    let trait_name = impl_item
                        .trait_
                        .as_ref()
                        .map(|path| &path.id)
                        .and_then(|id| self.crate_data.paths.get(id))
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

    pub fn format_item(&self, item: &Item) -> String {
        use crate::format::TypeFormatter;
        let kind = item_kind_str(&item.inner);
        let name = item.name.as_deref().unwrap_or("<unnamed>");
        let docs = item.docs.as_deref().unwrap_or("<no documentation>");

        let mut output = format!("{} {}\n", kind, name);

        let fmt = TypeFormatter::new(self);
        if matches!(item.inner, ItemEnum::Function(_)) {
            let _ = fmt.write_function_signature(&mut output, item);
            output.push('\n');
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
        if let Some(summary) = self.crate_data.paths.get(&item.id) {
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
}
