//! Search relevance and ranking algorithms.
//!
//! This module provides utilities for canonicality scoring used in search and query resolution.

/// Calculate a canonicality score for a path.
///
/// More canonical paths (shorter, fewer internal markers) get higher scores.
/// This helps prioritize public, stable API paths over internal re-exports.
///
/// Scoring:
/// - Base score: 100
/// - Penalty: -8 per additional path segment (beyond the first)
/// - Penalty: -40 for internal markers (_core, _private, _internal, __, etc.)
pub(crate) fn path_canonicality_score(path: &str) -> i32 {
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
