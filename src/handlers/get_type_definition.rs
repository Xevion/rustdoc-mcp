use crate::doc::DocIndex;
use crate::handlers::legacy;
use crate::types::ItemKind;
use rustdoc_types::{Generics, Id, Item, ItemEnum};

#[derive(Debug, Clone)]
pub struct TypeDefinition {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub fields: Option<Vec<FieldInfo>>,
    pub variants: Option<Vec<VariantInfo>>,
    pub docs: Option<String>,
    pub generics: Generics,
    pub item_id: Id,
    pub source_crate: String,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub docs: Option<String>,
    pub visibility: String,
}

#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub name: String,
    pub docs: Option<String>,
    /// Tuple variant fields: e.g., Some(T) -> vec!["T"]
    pub tuple_fields: Option<Vec<String>>,
    /// Struct variant fields: e.g., Point { x: i32, y: i32 }
    pub struct_fields: Option<Vec<FieldInfo>>,
}

pub async fn handle(
    query: &str,
    crates: Option<Vec<String>>,
    limit: Option<usize>,
) -> Result<(Vec<TypeDefinition>, Vec<(String, DocIndex)>), Box<dyn std::error::Error>> {
    let limit = limit.unwrap_or(5);

    // Resolve and load crates
    let crate_list = legacy::resolve_crates(crates.map(|c| c.join(",")))?;
    let (loaded_crates, _) = legacy::load_multiple_crates(&crate_list).await;

    if loaded_crates.is_empty() {
        return Err("No crates could be loaded".into());
    }

    // Search for types (Struct, Enum, Union)
    let struct_results = legacy::search_multiple_crates(&loaded_crates, query, Some(ItemKind::Struct));
    let enum_results = legacy::search_multiple_crates(&loaded_crates, query, Some(ItemKind::Enum));

    let mut all_results = Vec::new();
    all_results.extend(struct_results);
    all_results.extend(enum_results);

    // Sort by relevance
    all_results.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then_with(|| a.name.cmp(&b.name))
    });

    // Extract type definitions
    let mut definitions = Vec::new();

    for result in all_results.iter().take(limit) {
        if let Some(source_crate) = &result.source_crate
            && let Some((_, doc)) = loaded_crates.iter().find(|(name, _)| name == source_crate)
                && let Some(id) = &result.id
                    && let Some(item) = doc.get_item(id)
                        && let Some(def) = extract_type_definition(item, doc, source_crate.clone()) {
                            definitions.push(def);
                        }
    }

    Ok((definitions, loaded_crates))
}

fn extract_type_definition(item: &Item, doc: &DocIndex, source_crate: String) -> Option<TypeDefinition> {
    let name = item.name.as_ref()?.clone();
    let docs = item.docs.clone();
    let path = doc.get_item_path(item);
    let item_id = item.id;

    match &item.inner {
        ItemEnum::Struct(s) => {
            let fields = extract_struct_fields(&s.kind, doc);
            Some(TypeDefinition {
                name,
                kind: "struct".to_string(),
                path,
                fields: Some(fields),
                variants: None,
                docs,
                generics: s.generics.clone(),
                item_id,
                source_crate,
            })
        }
        ItemEnum::Enum(e) => {
            let variants = extract_enum_variants(&e.variants, doc);
            Some(TypeDefinition {
                name,
                kind: "enum".to_string(),
                path,
                fields: None,
                variants: Some(variants),
                docs,
                generics: e.generics.clone(),
                item_id,
                source_crate,
            })
        }
        ItemEnum::Union(u) => {
            let fields = extract_union_fields(&u.fields, doc);
            Some(TypeDefinition {
                name,
                kind: "union".to_string(),
                path,
                fields: Some(fields),
                variants: None,
                docs,
                generics: u.generics.clone(),
                item_id,
                source_crate,
            })
        }
        _ => None,
    }
}

fn extract_struct_fields(kind: &rustdoc_types::StructKind, doc: &DocIndex) -> Vec<FieldInfo> {
    match kind {
        rustdoc_types::StructKind::Plain { fields, .. } => {
            fields
                .iter()
                .filter_map(|field_id| {
                    let field_item = doc.get_item(field_id)?;

                    // Only include public fields
                    if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                        return None;
                    }

                    if let ItemEnum::StructField(ty) = &field_item.inner {
                        Some(FieldInfo {
                            name: field_item.name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                            type_name: doc.format_type(ty),
                            docs: field_item.docs.clone(),
                            visibility: "pub".to_string(),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        rustdoc_types::StructKind::Tuple(fields) => {
            fields
                .iter()
                .enumerate()
                .filter_map(|(idx, field_id_opt)| {
                    let field_id = field_id_opt.as_ref()?;
                    let field_item = doc.get_item(field_id)?;

                    // Only include public fields
                    if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                        return None;
                    }

                    if let ItemEnum::StructField(ty) = &field_item.inner {
                        Some(FieldInfo {
                            name: idx.to_string(),
                            type_name: doc.format_type(ty),
                            docs: field_item.docs.clone(),
                            visibility: "pub".to_string(),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        rustdoc_types::StructKind::Unit => Vec::new(),
    }
}

fn extract_union_fields(fields: &[rustdoc_types::Id], doc: &DocIndex) -> Vec<FieldInfo> {
    fields
        .iter()
        .filter_map(|field_id| {
            let field_item = doc.get_item(field_id)?;

            // Only include public fields
            if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                return None;
            }

            if let ItemEnum::StructField(ty) = &field_item.inner {
                Some(FieldInfo {
                    name: field_item.name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                    type_name: doc.format_type(ty),
                    docs: field_item.docs.clone(),
                    visibility: "pub".to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn extract_enum_variants(variants: &[rustdoc_types::Id], doc: &DocIndex) -> Vec<VariantInfo> {
    variants
        .iter()
        .filter_map(|variant_id| {
            let variant_item = doc.get_item(variant_id)?;

            if let ItemEnum::Variant(v) = &variant_item.inner {
                let name = variant_item.name.clone().unwrap_or_else(|| "<unnamed>".to_string());
                let docs = variant_item.docs.clone();

                let (tuple_fields, struct_fields) = match &v.kind {
                    rustdoc_types::VariantKind::Plain => (None, None),
                    rustdoc_types::VariantKind::Tuple(fields) => {
                        let tuple = fields
                            .iter()
                            .filter_map(|field_id_opt| {
                                let field_id = field_id_opt.as_ref()?;
                                let field_item = doc.get_item(field_id)?;
                                if let ItemEnum::StructField(ty) = &field_item.inner {
                                    Some(doc.format_type(ty))
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();
                        (Some(tuple), None)
                    }
                    rustdoc_types::VariantKind::Struct { fields, .. } => {
                        let strukt = fields
                            .iter()
                            .filter_map(|field_id| {
                                let field_item = doc.get_item(field_id)?;

                                // Only include public fields
                                if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                                    return None;
                                }

                                if let ItemEnum::StructField(ty) = &field_item.inner {
                                    Some(FieldInfo {
                                        name: field_item.name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                                        type_name: doc.format_type(ty),
                                        docs: field_item.docs.clone(),
                                        visibility: "pub".to_string(),
                                    })
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>();
                        (None, Some(strukt))
                    }
                };

                Some(VariantInfo {
                    name,
                    docs,
                    tuple_fields,
                    struct_fields,
                })
            } else {
                None
            }
        })
        .collect()
}
