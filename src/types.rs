use rustdoc_types::{Id, ItemEnum};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub crate_name: Option<String>,
    pub docs: Option<String>,
    pub id: Option<Id>,
    pub relevance: u32,
    pub source_crate: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraitImplInfo {
    pub trait_name: Option<String>,
    pub methods: Vec<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

pub fn matches_kind(inner: &ItemEnum, kind: ItemKind) -> bool {
    match (inner, kind) {
        (ItemEnum::Module(_), ItemKind::Module) => true,
        (ItemEnum::Struct(_), ItemKind::Struct) => true,
        (ItemEnum::Enum(_), ItemKind::Enum) => true,
        (ItemEnum::Function(_), ItemKind::Function) => true,
        (ItemEnum::Trait(_), ItemKind::Trait) => true,
        (ItemEnum::TypeAlias(_), ItemKind::TypeAlias) => true,
        (ItemEnum::Constant { .. }, ItemKind::Constant) => true,
        (ItemEnum::Static(_), ItemKind::Static) => true,
        _ => false,
    }
}

pub fn item_kind_str(inner: &ItemEnum) -> &'static str {
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

pub fn extract_id_from_type(ty: &rustdoc_types::Type) -> Option<&Id> {
    match ty {
        rustdoc_types::Type::ResolvedPath(path) => Some(&path.id),
        _ => None,
    }
}

pub fn calculate_relevance(text: &str, query: &str) -> Option<u32> {
    if text == query {
        Some(100)
    } else if text.starts_with(query) {
        Some(50)
    } else if text.contains(query) {
        Some(10)
    } else {
        None
    }
}

pub fn format_generic_param(param: &rustdoc_types::GenericParamDef) -> String {
    match &param.kind {
        rustdoc_types::GenericParamDefKind::Type { .. } => param.name.clone(),
        rustdoc_types::GenericParamDefKind::Lifetime { .. } => param.name.clone(),
        rustdoc_types::GenericParamDefKind::Const { .. } => param.name.clone(),
    }
}

pub fn path_canonicality_score(path: &str) -> i32 {
    let segments: Vec<&str> = path.split("::").collect();
    let mut score = 100;

    score -= (segments.len() as i32 - 1) * 10;

    let internal_markers = [
        "_core",
        "_private",
        "_internal",
        "internal",
        "private",
        "__",
    ];
    for segment in &segments {
        for marker in &internal_markers {
            if segment.contains(marker) {
                score -= 50;
                break;
            }
        }
    }

    score
}
