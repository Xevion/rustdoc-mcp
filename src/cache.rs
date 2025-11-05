//! Documentation caching with fingerprint-based regeneration.
//!
//! This module provides hash types and digest computation for tracking when
//! documentation needs regeneration based on file changes, version updates, or toolchain changes.

use crate::error::Result;
use anyhow::Context;
use ignore::WalkBuilder;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash as StdHash, Hasher};
use std::path::Path;
use std::str::FromStr;

/// Type-safe representation of hash values used throughout rustdoc-mcp.
///
/// Provides unified handling of different hash types with proper serialization,
/// validation, and display formatting.
#[derive(Copy, Clone, PartialEq, Eq, StdHash, Debug)]
pub enum Hash {
    /// SHA-256 hash (32 bytes), typically from Cargo.lock checksums
    Sha256([u8; 32]),
    /// 64-bit hash from DefaultHasher or AHasher
    U64(u64),
}

impl Hash {
    /// Create a SHA-256 hash from a byte array
    pub const fn sha256(bytes: [u8; 32]) -> Self {
        Hash::Sha256(bytes)
    }

    /// Create a U64 hash from a u64 value
    pub const fn u64(value: u64) -> Self {
        Hash::U64(value)
    }

    /// Returns the hash as a lowercase hexadecimal string
    pub fn as_hex(&self) -> String {
        match self {
            Hash::Sha256(bytes) => bytes.iter().map(|b| format!("{:02x}", b)).collect(),
            Hash::U64(value) => format!("{:016x}", value),
        }
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_hex())
    }
}

impl FromStr for Hash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim();

        // SHA-256 hashes are 64 hex characters (32 bytes * 2)
        if s.len() == 64 {
            let mut bytes = [0u8; 32];
            for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
                let hex_str = std::str::from_utf8(chunk).map_err(|_| ParseHashError::InvalidHex)?;
                bytes[i] =
                    u8::from_str_radix(hex_str, 16).map_err(|_| ParseHashError::InvalidHex)?;
            }
            Ok(Hash::Sha256(bytes))
        }
        // U64 hashes are 16 hex characters (8 bytes * 2)
        else if s.len() == 16 {
            let value = u64::from_str_radix(s, 16).map_err(|_| ParseHashError::InvalidHex)?;
            Ok(Hash::U64(value))
        } else {
            Err(ParseHashError::InvalidLength(s.len()))
        }
    }
}

impl Serialize for Hash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Error type for hash parsing failures
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseHashError {
    /// Invalid hexadecimal characters in the input
    InvalidHex,
    /// Invalid length (expected 16 or 64 hex characters)
    InvalidLength(usize),
}

impl fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseHashError::InvalidHex => {
                write!(f, "invalid hexadecimal characters in hash string")
            }
            ParseHashError::InvalidLength(len) => {
                write!(
                    f,
                    "invalid hash length: expected 16 or 64 hex characters, got {}",
                    len
                )
            }
        }
    }
}

impl std::error::Error for ParseHashError {}

/// Digest for tracking when documentation needs regeneration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateDigest {
    /// Hash of rustc version output (invalidates all docs on toolchain change)
    pub rustc_version_hash: u64,
    /// Type-specific digest data
    pub crate_type: DigestVariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DigestVariant {
    WorkspaceMember {
        /// Hash of Cargo.toml contents
        manifest_hash: u64,
        /// Combined hash of all source files
        source_hash: u64,
        /// Sorted list of enabled features
        features: Vec<String>,
    },
    Dependency {
        /// Crate version
        version: String,
        /// SHA256 checksum from Cargo.lock (guarantees immutability)
        checksum: Hash,
    },
}

/// Computes a digest for a workspace member based on manifest and source file contents.
/// Regeneration is triggered by changes to Cargo.toml, any .rs file, or rustc version.
pub async fn compute_workspace_digest(
    _crate_name: &str,
    workspace_root: &Path,
) -> Result<CrateDigest> {
    let rustc_version_hash = get_rustc_version_hash().await?;

    // Hash Cargo.toml
    let manifest_path = workspace_root.join("Cargo.toml");
    let manifest_hash = hash_file(&manifest_path)
        .await
        .with_context(|| format!("Failed to hash Cargo.toml at {}", manifest_path.display()))?;

    // Hash all source files
    let src_dir = workspace_root.join("src");
    let source_hash = hash_directory(&src_dir)
        .await
        .with_context(|| format!("Failed to hash source directory at {}", src_dir.display()))?;

    // For now, we don't track features (would need to be passed in)
    // This is acceptable because feature changes usually require explicit cargo invocations
    let features = Vec::new();

    Ok(CrateDigest {
        rustc_version_hash,
        crate_type: DigestVariant::WorkspaceMember {
            manifest_hash,
            source_hash,
            features,
        },
    })
}

/// Computes a digest for an external dependency using its version and Cargo.lock checksum.
/// Regeneration is triggered only by version changes or rustc updates.
pub async fn compute_dependency_digest(
    _crate_name: &str,
    version: &str,
    checksum: Hash,
) -> Result<CrateDigest> {
    let rustc_version_hash = get_rustc_version_hash().await?;

    Ok(CrateDigest {
        rustc_version_hash,
        crate_type: DigestVariant::Dependency {
            version: version.to_string(),
            checksum,
        },
    })
}

/// Loads a previously saved digest from disk.
pub async fn load_digest(path: &Path) -> Option<CrateDigest> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&content).ok()
}

/// Saves a digest to disk, creating parent directories if needed.
pub async fn save_digest(path: &Path, digest: &CrateDigest) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(digest).context("Failed to serialize digest")?;
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("Failed to write digest to {}", path.display()))?;
    Ok(())
}

/// Hashes the rustc version output to invalidate caches on toolchain changes.
async fn get_rustc_version_hash() -> Result<u64> {
    let output = tokio::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .await
        .context("Failed to execute rustc command")?;

    if !output.status.success() {
        anyhow::bail!("Failed to get rustc version");
    }

    let version_string = String::from_utf8(output.stdout)
        .context("Failed to parse rustc version output as UTF-8")?;
    let mut hasher = DefaultHasher::new();
    version_string.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Hashes a single file's contents.
async fn hash_file(path: &Path) -> Result<u64> {
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read file {}", path.display()))?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(hasher.finish())
}

/// Recursively hashes all .rs files in a directory in deterministic order.
/// Uses relative paths to ensure digests survive project moves.
async fn hash_directory(dir: &Path) -> Result<u64> {
    let dir = dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut hasher = DefaultHasher::new();

        // Walk directory in sorted order for deterministic hashing
        let mut entries: Vec<_> = WalkBuilder::new(&dir)
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext == "rs")
                    .unwrap_or(false)
            })
            .collect();

        entries.sort_by(|a, b| a.path().cmp(b.path()));

        for entry in entries {
            let path = entry.path();

            // Hash the relative path (so digest survives project moves)
            if let Ok(rel_path) = path.strip_prefix(&dir) {
                rel_path.to_string_lossy().hash(&mut hasher);
            }

            // Hash the file contents
            if let Ok(content) = std::fs::read_to_string(path) {
                content.hash(&mut hasher);
            }
        }

        Ok(hasher.finish())
    })
    .await
    .context("Task panicked")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::{check, let_assert};
    use rstest::rstest;

    #[rstest]
    #[case(
        "a3b2c1d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2",
        true
    )]
    #[case(
        "0000000000000000000000000000000000000000000000000000000000000000",
        true
    )]
    #[case(
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        true
    )]
    fn test_sha256_parsing(#[case] hash_str: &str, #[case] should_succeed: bool) {
        let result = hash_str.parse::<Hash>();
        check!(result.is_ok() == should_succeed);

        if should_succeed {
            let hash = result.unwrap();
            check!(matches!(hash, Hash::Sha256(_)));
            check!(hash.to_string() == hash_str);
        }
    }

    #[rstest]
    #[case("123456789abcdef0", 0x123456789abcdef0)]
    #[case("0000000000000000", 0)]
    #[case("ffffffffffffffff", u64::MAX)]
    #[case("00000000000000ff", 255)]
    fn test_u64_parsing(#[case] hash_str: &str, #[case] expected: u64) {
        let hash: Hash = hash_str.parse().unwrap();
        check!(hash == Hash::U64(expected));
        check!(hash.to_string() == hash_str);
    }

    #[rstest]
    #[case("zzzzzzzzzzzzzzzz", ParseHashError::InvalidHex)]
    #[case("GGGGGGGGGGGGGGGG", ParseHashError::InvalidHex)]
    #[case("123456789abcdefg", ParseHashError::InvalidHex)]
    fn test_invalid_hex(#[case] input: &str, #[case] expected_error: ParseHashError) {
        let result = input.parse::<Hash>();
        let_assert!(Err(err) = result);
        check!(err == expected_error);
    }

    #[rstest]
    #[case("", 0)]
    #[case("abc123", 6)]
    #[case("12345678", 8)]
    #[case("1234567890abcdef0", 17)]
    #[case("abc", 3)]
    fn test_invalid_length(#[case] input: &str, #[case] len: usize) {
        let result = input.parse::<Hash>();
        let_assert!(Err(ParseHashError::InvalidLength(actual_len)) = result);
        check!(actual_len == len);
    }

    #[rstest]
    #[case(Hash::Sha256([0xaa; 32]), "\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"")]
    #[case(Hash::Sha256([0x00; 32]), "\"0000000000000000000000000000000000000000000000000000000000000000\"")]
    #[case(Hash::Sha256([0xff; 32]), "\"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"")]
    fn test_serialization_sha256(#[case] hash: Hash, #[case] expected_json: &str) {
        let json = serde_json::to_string(&hash).unwrap();
        check!(json == expected_json);

        let deserialized: Hash = serde_json::from_str(&json).unwrap();
        check!(hash == deserialized);
    }

    #[rstest]
    #[case(Hash::U64(0x123456789abcdef0), "\"123456789abcdef0\"")]
    #[case(Hash::U64(0), "\"0000000000000000\"")]
    #[case(Hash::U64(u64::MAX), "\"ffffffffffffffff\"")]
    #[case(Hash::U64(255), "\"00000000000000ff\"")]
    fn test_serialization_u64(#[case] hash: Hash, #[case] expected_json: &str) {
        let json = serde_json::to_string(&hash).unwrap();
        check!(json == expected_json);

        let deserialized: Hash = serde_json::from_str(&json).unwrap();
        check!(hash == deserialized);
    }

    #[rstest]
    #[case(Hash::Sha256([0x0f; 32]), 64)]
    #[case(Hash::Sha256([0x00; 32]), 64)]
    #[case(Hash::Sha256([0xff; 32]), 64)]
    #[case(Hash::U64(255), 16)]
    #[case(Hash::U64(0), 16)]
    #[case(Hash::U64(u64::MAX), 16)]
    fn test_display_formatting(#[case] hash: Hash, #[case] expected_len: usize) {
        let display_str = hash.to_string();
        check!(display_str.len() == expected_len);
        check!(
            display_str
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    #[test]
    fn test_equality() {
        let hash1 = Hash::Sha256([1; 32]);
        let hash2 = Hash::Sha256([1; 32]);
        let hash3 = Hash::Sha256([2; 32]);
        let hash4 = Hash::U64(123);
        let hash5 = Hash::U64(123);

        check!(hash1 == hash2);
        check!(hash1 != hash3);
        check!(hash1 != hash4);
        check!(hash4 == hash5);
    }

    #[test]
    fn test_copy_trait() {
        let hash = Hash::U64(42);
        let hash_copy = hash;
        check!(hash == hash_copy);
    }

    #[tokio::test]
    async fn test_rustc_version_hash() {
        let hash = get_rustc_version_hash()
            .await
            .expect("Failed to get rustc version");
        check!(hash > 0);
    }
}
