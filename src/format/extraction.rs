use crate::format::TypeFormatter;
use crate::search::CrateIndex;
use crate::types::{CrateName, TypeKind, Visibility};
use rustdoc_types::{Generics, Id, Item, ItemEnum};

#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub path: String,
    pub fields: Option<Vec<FieldInfo>>,
    pub variants: Option<Vec<VariantInfo>>,
    pub docs: Option<String>,
    pub generics: Generics,
    pub item_id: Id,
    pub source_crate: CrateName,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub docs: Option<String>,
    pub visibility: Visibility,
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

/// Extracts type information (struct/enum/union) from a rustdoc Item.
/// Returns None for non-type items. Used by type formatting to generate Rust syntax.
pub fn extract_type_definition(
    item: &Item,
    index: &CrateIndex,
    source_crate: CrateName,
) -> Option<TypeInfo> {
    let name = item.name.as_ref()?.clone();
    let docs = item.docs.clone();
    let path = index.get_item_path(item);
    let item_id = item.id;

    match &item.inner {
        ItemEnum::Struct(s) => {
            let fields = extract_struct_fields(&s.kind, index);
            Some(TypeInfo {
                name,
                kind: TypeKind::Struct,
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
            let variants = extract_enum_variants(&e.variants, index);
            Some(TypeInfo {
                name,
                kind: TypeKind::Enum,
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
            let fields = extract_union_fields(&u.fields, index);
            Some(TypeInfo {
                name,
                kind: TypeKind::Union,
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

/// Extracts public fields from a struct, handling plain/tuple/unit structs.
fn extract_struct_fields(kind: &rustdoc_types::StructKind, index: &CrateIndex) -> Vec<FieldInfo> {
    match kind {
        rustdoc_types::StructKind::Plain { fields, .. } => {
            fields
                .iter()
                .filter_map(|field_id| {
                    let field_item = index.get_item(field_id)?;

                    // Only include public fields
                    if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                        return None;
                    }

                    if let ItemEnum::StructField(ty) = &field_item.inner {
                        Some(FieldInfo {
                            name: field_item
                                .name
                                .clone()
                                .unwrap_or_else(|| "<unnamed>".to_string()),
                            type_name: index.format_type(ty),
                            docs: field_item.docs.clone(),
                            visibility: Visibility::Public,
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
                    let field_item = index.get_item(field_id)?;

                    // Only include public fields
                    if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                        return None;
                    }

                    if let ItemEnum::StructField(ty) = &field_item.inner {
                        Some(FieldInfo {
                            name: idx.to_string(),
                            type_name: index.format_type(ty),
                            docs: field_item.docs.clone(),
                            visibility: Visibility::Public,
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

/// Extracts public fields from a union.
fn extract_union_fields(fields: &[rustdoc_types::Id], index: &CrateIndex) -> Vec<FieldInfo> {
    fields
        .iter()
        .filter_map(|field_id| {
            let field_item = index.get_item(field_id)?;

            // Only include public fields
            if !matches!(field_item.visibility, rustdoc_types::Visibility::Public) {
                return None;
            }

            if let ItemEnum::StructField(ty) = &field_item.inner {
                Some(FieldInfo {
                    name: field_item
                        .name
                        .clone()
                        .unwrap_or_else(|| "<unnamed>".to_string()),
                    type_name: index.format_type(ty),
                    docs: field_item.docs.clone(),
                    visibility: Visibility::Public,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Extracts variants from an enum, handling plain/tuple/struct variants.
fn extract_enum_variants(variants: &[rustdoc_types::Id], index: &CrateIndex) -> Vec<VariantInfo> {
    variants
        .iter()
        .filter_map(|variant_id| {
            let variant_item = index.get_item(variant_id)?;

            if let ItemEnum::Variant(v) = &variant_item.inner {
                let name = variant_item
                    .name
                    .clone()
                    .unwrap_or_else(|| "<unnamed>".to_string());
                let docs = variant_item.docs.clone();

                let (tuple_fields, struct_fields) = match &v.kind {
                    rustdoc_types::VariantKind::Plain => (None, None),
                    rustdoc_types::VariantKind::Tuple(fields) => {
                        let tuple = fields
                            .iter()
                            .filter_map(|field_id_opt| {
                                let field_id = field_id_opt.as_ref()?;
                                let field_item = index.get_item(field_id)?;
                                if let ItemEnum::StructField(ty) = &field_item.inner {
                                    Some(index.format_type(ty))
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
                                let field_item = index.get_item(field_id)?;

                                // Only include public fields
                                if !matches!(
                                    field_item.visibility,
                                    rustdoc_types::Visibility::Public
                                ) {
                                    return None;
                                }

                                if let ItemEnum::StructField(ty) = &field_item.inner {
                                    Some(FieldInfo {
                                        name: field_item
                                            .name
                                            .clone()
                                            .unwrap_or_else(|| "<unnamed>".to_string()),
                                        type_name: index.format_type(ty),
                                        docs: field_item.docs.clone(),
                                        visibility: Visibility::Public,
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
