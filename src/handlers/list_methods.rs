
#[derive(Debug, Clone)]
pub struct MethodList {
    pub type_name: String,
    pub type_path: String,
    pub inherent_methods: Vec<MethodInfo>,
    pub trait_methods: Vec<TraitMethodGroup>,
}

#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub name: String,
    pub signature: String,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraitMethodGroup {
    pub trait_name: String,
    pub methods: Vec<MethodInfo>,
}

pub async fn handle(
    query: &str,
    crates: Option<Vec<String>>,
) -> Result<Vec<MethodList>, Box<dyn std::error::Error>> {
    // TODO: Implementation
    // 1. Load crates
    // 2. Search for types matching query (fuzzy)
    // 3. For each type, find all impl blocks
    // 4. Separate inherent impls from trait impls
    // 5. Extract method signatures from each impl
    // 6. Return structured MethodList data
    todo!("Implement list_methods handler")
}
