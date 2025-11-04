/// Represents a parsed item path like `std::vec::Vec` or `MyStruct`
#[derive(Debug, Clone)]
pub struct ItemPath {
    /// The crate name if explicitly specified or resolved
    pub crate_name: Option<String>,
    /// Path components (modules and item name)
    pub path_components: Vec<String>,
}

impl ItemPath {
    /// Get the final item name (last component)
    pub fn item_name(&self) -> &str {
        self.path_components
            .last()
            .expect("ItemPath must have at least one component")
    }

    /// Get the module path without the item name
    pub fn module_path(&self) -> Option<String> {
        if self.path_components.len() > 1 {
            Some(
                self.path_components[..self.path_components.len() - 1].join("::")
            )
        } else {
            None
        }
    }

    /// Get the full path including module and item
    pub fn full_path(&self) -> String {
        self.path_components.join("::")
    }

    /// Get the fully qualified path including crate name
    pub fn qualified_path(&self) -> String {
        if let Some(ref crate_name) = self.crate_name {
            format!("{}::{}", crate_name, self.full_path())
        } else {
            self.full_path()
        }
    }
}

/// Parse an item path query into components
///
/// Examples:
/// - `Vec` → path_components=["Vec"]
/// - `std::vec::Vec` → path_components=["std", "vec", "Vec"]
/// - `collections::HashMap` → path_components=["collections", "HashMap"]
///
/// The crate name is resolved later with context knowledge of available crates
pub fn parse_item_path(query: &str) -> ItemPath {
    let parts: Vec<String> = query
        .split("::")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        // Handle empty query
        ItemPath {
            crate_name: None,
            path_components: vec!["".to_string()],
        }
    } else {
        ItemPath {
            crate_name: None,
            path_components: parts,
        }
    }
}

/// Attempt to resolve the crate name from the path using known crates
///
/// If the first component matches a known crate name, it's extracted as the crate
/// and removed from the path components.
///
/// Returns the resolved crate name if found.
pub fn resolve_crate_from_path(
    path: &mut ItemPath,
    known_crates: &[String],
) -> Option<String> {
    if path.path_components.is_empty() {
        return None;
    }

    let first = &path.path_components[0];
    if known_crates.iter().any(|c| c == first) {
        // First component matches a known crate
        let crate_name = path.path_components.remove(0);
        path.crate_name = Some(crate_name.clone());
        Some(crate_name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_item() {
        let path = parse_item_path("Vec");
        assert_eq!(path.item_name(), "Vec");
        assert_eq!(path.module_path(), None);
        assert_eq!(path.full_path(), "Vec");
        assert!(path.crate_name.is_none());
    }

    #[test]
    fn test_parse_qualified_path() {
        let path = parse_item_path("std::vec::Vec");
        assert_eq!(path.item_name(), "Vec");
        assert_eq!(path.full_path(), "std::vec::Vec");
        assert!(path.crate_name.is_none()); // Not yet resolved
    }

    #[test]
    fn test_resolve_crate_name() {
        let mut path = parse_item_path("std::vec::Vec");
        let known_crates = vec!["std".to_string(), "tokio".to_string()];

        let crate_name = resolve_crate_from_path(&mut path, &known_crates);

        assert_eq!(crate_name, Some("std".to_string()));
        assert_eq!(path.crate_name, Some("std".to_string()));
        assert_eq!(path.item_name(), "Vec");
        assert_eq!(path.module_path(), Some("vec".to_string()));
    }

    #[test]
    fn test_no_crate_resolution() {
        let mut path = parse_item_path("collections::HashMap");
        let known_crates = vec!["std".to_string(), "tokio".to_string()];

        let crate_name = resolve_crate_from_path(&mut path, &known_crates);

        assert_eq!(crate_name, None);
        assert!(path.crate_name.is_none());
        assert_eq!(path.item_name(), "HashMap");
    }
}
