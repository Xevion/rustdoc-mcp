//! Search relevance and ranking algorithms.
//!
//! This module provides utilities for canonicality scoring used in search and query resolution.

/// Convert a relevance score in `[0.0, 1.0]` to an integer percent in `[0, 100]`.
///
/// NaN maps to 0, scores outside the range are clamped. This is the display
/// format surfaced to MCP clients — precision beyond whole percent is noise.
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to [0, 100] before cast"
)]
pub(crate) fn score_to_percent(score: f32) -> u32 {
    if score.is_nan() {
        return 0;
    }
    let pct = (score * 100.0).clamp(0.0, 100.0);
    pct as u32
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
pub(crate) fn path_canonicality_score(path: &str) -> i32 {
    let segments: Vec<&str> = path.split("::").collect();
    let mut score = 100;

    // Penalize longer paths (path segment counts trivially fit in i32)
    let extra_segments = i32::try_from(segments.len().saturating_sub(1)).unwrap_or(i32::MAX);
    score -= extra_segments * 8;

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
