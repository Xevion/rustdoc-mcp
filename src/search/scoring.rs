//! Search relevance and ranking algorithms.
//!
//! This module provides utilities for calculating relevance scores, path matching,
//! and canonicality scoring used in search and query resolution.

/// Calculate simple text relevance score.
///
/// Returns a score based on how well the query matches the text:
/// - 100: Exact match
/// - 50: Text starts with query
/// - 10: Text contains query
/// - None: No match
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

/// Calculate relevance for path-aware queries.
///
/// Matches items where the path ends with the query components.
/// For example, query ["de", "deserialize"] matches paths like:
/// - ["serde_core", "de", "Deserialize"] (exact suffix match)
/// - ["serde", "de", "Deserialize"] (exact suffix match)
///
/// Returns:
/// - 100: Exact length match (path length == query length)
/// - 90: Suffix match (path is longer than query)
/// - None: No match
pub fn calculate_path_relevance(item_path: &[String], query_components: &[&str]) -> Option<u32> {
    if query_components.is_empty() {
        return None;
    }

    // Convert item path to lowercase for case-insensitive comparison
    let item_path_lower: Vec<String> = item_path.iter().map(|s| s.to_lowercase()).collect();

    // Check if the item path ends with the query components
    if item_path_lower.len() < query_components.len() {
        return None;
    }

    // Get the suffix of the item path that matches the query length
    let suffix = &item_path_lower[item_path_lower.len() - query_components.len()..];

    // Check if all components match
    let exact_match = suffix
        .iter()
        .zip(query_components.iter())
        .all(|(item_seg, query_seg)| item_seg == query_seg);

    if exact_match {
        // Prefer exact length matches over longer paths
        if item_path_lower.len() == query_components.len() {
            Some(100)
        } else {
            Some(90)
        }
    } else {
        None
    }
}

/// Calculate a canonicality score for a path.
///
/// More canonical paths (shorter, fewer internal markers) get higher scores.
/// This helps prioritize public, stable API paths over internal re-exports.
///
/// Scoring:
/// - Base score: 100
/// - Penalty: -8 per additional path segment (beyond the first)
/// - Penalty: -40 for internal markers (_core, _private, _internal, __, etc.)
pub fn path_canonicality_score(path: &str) -> i32 {
    let segments: Vec<&str> = path.split("::").collect();
    let mut score = 100;

    // Penalize longer paths
    score -= (segments.len() as i32 - 1) * 8;

    // Penalize internal markers
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
                score -= 40;
                break;
            }
        }
    }

    score
}
