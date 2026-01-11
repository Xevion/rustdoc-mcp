//! Domain-specific types to replace primitive string obsession.
//!
//! This module provides strongly-typed alternatives to raw strings for:
//! - Type kinds (struct, enum, union)
//! - Field visibility
//! - Crate names (with validation and normalization)

use serde::{Deserialize, Serialize};
use std::borrow::{Borrow, Cow};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Kind of type definition (struct, enum, or union).
///
/// Replaces the previous `kind: String` field in `TypeInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    Struct,
    Enum,
    Union,
}

impl TypeKind {
    /// Returns the Rust keyword for this type kind.
    #[inline]
    pub fn keyword(&self) -> &'static str {
        match self {
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Union => "union",
        }
    }
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// Visibility of a field or item.
///
/// Currently only `Public` is used (non-public fields are filtered out),
/// but this enum allows future extensibility for `pub(crate)`, `pub(super)`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    // Future variants:
    // Crate,
    // Restricted(Path),
    // Private,
}

impl Visibility {
    /// Returns the Rust keyword for this visibility.
    #[inline]
    pub fn keyword(&self) -> &'static str {
        match self {
            Self::Public => "pub",
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// A validated, normalized crate name.
///
/// Crate names in Rust can contain alphanumeric characters, hyphens, and underscores.
/// However, in the Rust module system (and in rustdoc JSON filenames), hyphens are
/// normalized to underscores.
///
/// This type stores both the normalized form (for lookups) and the original form
/// (for display purposes).
///
/// # Examples
///
/// ```
/// use rustdoc_mcp::types::CrateName;
///
/// let name = CrateName::new("serde-json").unwrap();
/// assert_eq!(name.normalized(), "serde_json");
/// assert_eq!(name.as_str(), "serde-json");
/// assert!(name.matches("serde_json"));
/// assert!(name.matches("serde-json"));
/// ```
#[derive(Debug, Clone)]
pub struct CrateName {
    /// Normalized name (hyphens replaced with underscores)
    normalized: String,
    /// Original name as provided (for display)
    original: String,
}

impl CrateName {
    /// Create a new CrateName from a string.
    ///
    /// Returns an error if the name is invalid (empty or contains invalid characters).
    pub fn new(name: impl Into<String>) -> Result<Self, CrateNameError> {
        let original = name.into();
        Self::validate(&original)?;
        let normalized = original.replace('-', "_");
        Ok(Self {
            normalized,
            original,
        })
    }

    /// Create a CrateName without validation.
    ///
    /// Use this for crate names from trusted sources (e.g., cargo metadata output).
    #[inline]
    pub fn new_unchecked(name: impl Into<String>) -> Self {
        let original = name.into();
        let normalized = original.replace('-', "_");
        Self {
            normalized,
            original,
        }
    }

    /// Normalize a crate name string (hyphens → underscores).
    ///
    /// Returns a borrowed string if no normalization is needed (no hyphens present),
    /// otherwise returns an owned string with hyphens replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use rustdoc_mcp::types::CrateName;
    /// use std::borrow::Cow;
    ///
    /// // No allocation when no hyphens
    /// assert!(matches!(CrateName::normalize("serde"), Cow::Borrowed(_)));
    ///
    /// // Allocates when hyphens present
    /// assert_eq!(CrateName::normalize("serde-json"), "serde_json");
    /// ```
    pub fn normalize(name: &str) -> Cow<'_, str> {
        if name.contains('-') {
            Cow::Owned(name.replace('-', "_"))
        } else {
            Cow::Borrowed(name)
        }
    }

    /// Validate a crate name according to Rust/Cargo naming rules.
    fn validate(name: &str) -> Result<(), CrateNameError> {
        if name.is_empty() {
            return Err(CrateNameError::Empty);
        }

        let mut chars = name.chars();

        // First character must be a letter or underscore
        let first = chars.next().unwrap();
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(CrateNameError::InvalidStart(first));
        }

        // Rest must be alphanumeric, underscore, or hyphen
        for ch in chars {
            if !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' {
                return Err(CrateNameError::InvalidChar(ch));
            }
        }

        Ok(())
    }

    /// Get the normalized name (hyphens replaced with underscores).
    ///
    /// Use this for filesystem paths and module lookups.
    #[inline]
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Get the original name as provided.
    ///
    /// Use this for display purposes.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.original
    }

    /// Get the path to the rustdoc JSON file for this crate.
    #[inline]
    pub fn doc_json_path(&self, target_doc: &Path) -> PathBuf {
        target_doc.join(format!("{}.json", self.normalized))
    }

    /// Get the path to the search index file for this crate.
    #[inline]
    pub fn index_path(&self, target_doc: &Path) -> PathBuf {
        target_doc.join(format!("{}.index", self.normalized))
    }

    /// Check if this crate name matches another string (normalized comparison).
    ///
    /// Both names are normalized before comparison, so `serde-json` matches `serde_json`.
    /// This method avoids allocation by comparing byte-by-byte.
    #[inline]
    pub fn matches(&self, other: &str) -> bool {
        self.normalized.len() == other.len()
            && self
                .normalized
                .bytes()
                .zip(other.bytes())
                .all(|(a, b)| a == b || (a == b'_' && b == b'-'))
    }
}

// Equality based on normalized form
impl PartialEq for CrateName {
    fn eq(&self, other: &Self) -> bool {
        self.normalized == other.normalized
    }
}

impl Eq for CrateName {}

impl PartialOrd for CrateName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CrateName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.normalized.cmp(&other.normalized)
    }
}

impl Hash for CrateName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.normalized.hash(state);
    }
}

impl fmt::Display for CrateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.original)
    }
}

// Allow HashMap lookups with &str
impl Borrow<str> for CrateName {
    fn borrow(&self) -> &str {
        &self.normalized
    }
}

// Allow &str comparisons
impl PartialEq<str> for CrateName {
    fn eq(&self, other: &str) -> bool {
        self.matches(other)
    }
}

impl PartialEq<&str> for CrateName {
    fn eq(&self, other: &&str) -> bool {
        self.matches(other)
    }
}

impl PartialEq<String> for CrateName {
    fn eq(&self, other: &String) -> bool {
        self.matches(other)
    }
}

// Serialize as the original string
impl Serialize for CrateName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.original.serialize(serializer)
    }
}

// Deserialize with validation
impl<'de> Deserialize<'de> for CrateName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        CrateName::new(s).map_err(serde::de::Error::custom)
    }
}

/// Error type for invalid crate names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrateNameError {
    /// Crate name is empty.
    Empty,
    /// Crate name starts with an invalid character.
    InvalidStart(char),
    /// Crate name contains an invalid character.
    InvalidChar(char),
}

impl fmt::Display for CrateNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "crate name cannot be empty"),
            Self::InvalidStart(ch) => {
                write!(
                    f,
                    "crate name must start with a letter or underscore, got '{}'",
                    ch
                )
            }
            Self::InvalidChar(ch) => {
                write!(f, "crate name contains invalid character '{}'", ch)
            }
        }
    }
}

impl std::error::Error for CrateNameError {}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    #[test]
    fn test_type_kind_display() {
        check!(TypeKind::Struct.to_string() == "struct");
        check!(TypeKind::Enum.to_string() == "enum");
        check!(TypeKind::Union.to_string() == "union");
    }

    #[test]
    fn test_visibility_display() {
        check!(Visibility::Public.to_string() == "pub");
        check!(Visibility::default() == Visibility::Public);
    }

    #[test]
    fn test_crate_name_valid() {
        let name = CrateName::new("serde").unwrap();
        check!(name.as_str() == "serde");
        check!(name.normalized() == "serde");

        let name = CrateName::new("serde-json").unwrap();
        check!(name.as_str() == "serde-json");
        check!(name.normalized() == "serde_json");

        let name = CrateName::new("my_crate").unwrap();
        check!(name.as_str() == "my_crate");
        check!(name.normalized() == "my_crate");

        let name = CrateName::new("_private").unwrap();
        check!(name.as_str() == "_private");
    }

    #[test]
    fn test_crate_name_invalid() {
        check!(CrateName::new("").is_err());
        check!(CrateName::new("123abc").is_err());
        check!(CrateName::new("-invalid").is_err());
        check!(CrateName::new("has space").is_err());
        check!(CrateName::new("has.dot").is_err());
    }

    #[test]
    fn test_crate_name_equality() {
        let a = CrateName::new("serde-json").unwrap();
        let b = CrateName::new("serde_json").unwrap();
        check!(a == b);
        check!(a == "serde_json");
        check!(a == "serde-json");
    }

    #[test]
    fn test_crate_name_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(CrateName::new("serde-json").unwrap());

        // Should be found regardless of hyphen/underscore
        check!(set.contains(&CrateName::new("serde_json").unwrap()));
    }

    #[test]
    fn test_crate_name_paths() {
        let name = CrateName::new("serde-json").unwrap();
        let target_doc = Path::new("/project/target/doc");

        check!(
            name.doc_json_path(target_doc) == PathBuf::from("/project/target/doc/serde_json.json")
        );
        check!(
            name.index_path(target_doc) == PathBuf::from("/project/target/doc/serde_json.index")
        );
    }

    #[test]
    fn test_crate_name_unchecked() {
        // Should work even with unusual names (trusted source)
        let name = CrateName::new_unchecked("any-name");
        check!(name.normalized() == "any_name");
    }

    #[test]
    fn test_crate_name_normalize() {
        use std::borrow::Cow;

        // No allocation when no hyphens (returns borrowed)
        let result = CrateName::normalize("serde");
        check!(result == "serde");
        check!(matches!(result, Cow::Borrowed(_)));

        let result = CrateName::normalize("my_crate");
        check!(result == "my_crate");
        check!(matches!(result, Cow::Borrowed(_)));

        // Allocates when hyphens present (returns owned)
        let result = CrateName::normalize("serde-json");
        check!(result == "serde_json");
        check!(matches!(result, Cow::Owned(_)));

        let result = CrateName::normalize("my-awesome-crate");
        check!(result == "my_awesome_crate");
        check!(matches!(result, Cow::Owned(_)));
    }

    #[test]
    fn test_crate_name_matches_optimized() {
        let name = CrateName::new("serde-json").unwrap();

        // Should match both forms without allocation
        check!(name.matches("serde-json"));
        check!(name.matches("serde_json"));

        // Should not match different names
        check!(!name.matches("serde"));
        check!(!name.matches("serde-rs"));
        check!(!name.matches("serde_rs"));

        // Edge cases
        check!(!name.matches("serde-json-extra"));
        check!(!name.matches(""));

        // Name without hyphens
        let name = CrateName::new("tokio").unwrap();
        check!(name.matches("tokio"));
        check!(!name.matches("tokio-util"));
    }
}
