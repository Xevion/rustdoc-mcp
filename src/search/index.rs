//! TF-IDF inverted index implementation for full-text search.

// Query scoring multiplies usize match counts by f32 weights; precision loss
// beyond the mantissa limit is irrelevant for ranking.
#![allow(clippy::cast_precision_loss)]

use crate::item::ItemRef;
use crate::types::CrateName;
use postcard::{from_io, to_io};
use rustdoc_types::Item;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

use super::tokenize::{TermBuilder, hash_term, tokenize_and_stem};
use rust_stemmers::{Algorithm, Stemmer};

/// Process-wide counters for observing [`TermIndex`] cache behavior.
///
/// These atomics are incremented whenever the search index is loaded from
/// disk or rebuilt from scratch. Tests can read deltas (snapshot before,
/// snapshot after) to verify cache-reuse behavior without depending on
/// wall-clock timing or log parsing.
///
/// # Concurrency
///
/// Counters are process-global, so compare deltas, not absolute values. Under
/// nextest each test owns its process, so a delta may be asserted exactly.
pub mod metrics {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static INDEX_BUILDS: AtomicUsize = AtomicUsize::new(0);
    static INDEX_LOADS: AtomicUsize = AtomicUsize::new(0);

    /// Snapshot of `(builds, loads)` counters at a point in time.
    #[must_use]
    pub fn snapshot() -> (usize, usize) {
        (
            INDEX_BUILDS.load(Ordering::Relaxed),
            INDEX_LOADS.load(Ordering::Relaxed),
        )
    }

    pub(super) fn record_build() {
        INDEX_BUILDS.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_load() {
        INDEX_LOADS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Term hash for fast lookup
type TermHash = u64;

/// Magic bytes identifying a rustdoc-mcp search index cache file.
const INDEX_MAGIC: [u8; 4] = *b"RDMI";

/// Bump whenever [`InvertedIndex`] or the tokenizer behind its term hashes changes.
///
/// postcard is not self-describing, so an older cache decodes without error into an
/// index whose hashes match nothing, and every query silently misses.
const INDEX_SCHEMA_VERSION: u32 = 1;

/// Magic, schema version, rustdoc JSON format, and the source digest.
const INDEX_HEADER_LEN: usize = 20;

fn index_header(source_digest: u64) -> [u8; INDEX_HEADER_LEN] {
    let mut header = [0u8; INDEX_HEADER_LEN];
    header[0..4].copy_from_slice(&INDEX_MAGIC);
    header[4..8].copy_from_slice(&INDEX_SCHEMA_VERSION.to_le_bytes());
    header[8..12].copy_from_slice(&rustdoc_types::FORMAT_VERSION.to_le_bytes());
    header[12..20].copy_from_slice(&source_digest.to_le_bytes());
    header
}

/// Digest of the rustdoc JSON an index is built from, or `None` if unreadable.
///
/// Keyed on content, not mtime: a regenerated JSON can land just before an index
/// built from the previous content, leaving a stale index that looks fresh.
fn source_digest(path: &Path) -> Option<u64> {
    std::fs::read(path)
        .ok()
        .map(|bytes| xxhash_rust::xxh3::xxh3_64(&bytes))
}

/// Smallest multiplier a partial multi-word match can be scaled by.
///
/// The coverage factor interpolates from this floor (no query terms matched) up
/// to `1.0` (every term matched), so a document that matches a subset of a
/// multi-word query is always demoted relative to a full match but never driven
/// toward zero. Without the floor, a bare `(matched / total)^2` factor scales as
/// `1/N^2` in the query length: one term of a four-word query keeps 6% of its
/// score, which buries every partial match below the result limit even when no
/// document matches the query in full.
const COVERAGE_FLOOR: f32 = 0.2;

/// A searchable term index with TF-IDF scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InvertedIndex {
    /// Map from term hash to list of (crate_index, tf_idf_score) pairs, sorted by score descending
    terms: HashMap<TermHash, Vec<(usize, f32)>>,
    /// Map from crate_index to id_path (sequence of u32 IDs from root to item)
    ids: Vec<Vec<u32>>,
}

impl InvertedIndex {
    /// Create a new InvertedIndex with the given terms and document IDs
    pub(super) const fn new(
        terms: HashMap<TermHash, Vec<(usize, f32)>>,
        ids: Vec<Vec<u32>>,
    ) -> Self {
        Self { terms, ids }
    }

    /// Searches for items matching the query term using TF-IDF scoring.
    /// Returns item ID paths sorted by relevance score (highest first).
    ///
    /// The query is tokenized and stemmed just like indexed terms, so:
    /// - "BackgroundWorker" matches items with "background", "worker", or "backgroundwork"
    /// - CamelCase, snake_case, and hyphen-case are all handled
    ///
    /// Space-separated queries additionally scale each document by a coverage
    /// factor derived from how many of the query's terms it matched, bounded
    /// below by [`COVERAGE_FLOOR`] so partial matches rank below full ones
    /// without disappearing.
    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<(Vec<u32>, f32)> {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(query, &stemmer);

        if tokens.is_empty() {
            return vec![];
        }

        // Collect results from all tokens, combining scores for documents that match multiple.
        // Also track how many distinct tokens each document matched.
        let mut combined_scores: HashMap<usize, f32> = HashMap::new();
        let mut token_match_counts: HashMap<usize, usize> = HashMap::new();

        for token in &tokens {
            let term_hash = hash_term(token);
            if let Some(results) = self.terms.get(&term_hash) {
                for (doc_idx, score) in results {
                    *combined_scores.entry(*doc_idx).or_insert(0.0) += score;
                    *token_match_counts.entry(*doc_idx).or_insert(0) += 1;
                }
            }
        }

        // Only space-separated queries carry a coverage signal. Single identifiers like
        // "TypeFormatter" also tokenize into several terms, but they name one thing.
        let total_tokens = tokens.len() as f32;
        if query.contains(' ') && total_tokens > 1.0 {
            for (doc_idx, score) in &mut combined_scores {
                let matched = token_match_counts.get(doc_idx).copied().unwrap_or(0) as f32;
                let coverage = matched / total_tokens;
                let factor = (1.0 - COVERAGE_FLOOR).mul_add(coverage * coverage, COVERAGE_FLOOR);

                // TF-IDF aggregates go negative for terms that are common relative to
                // document length, so dividing keeps the demotion monotone for those.
                if *score >= 0.0 {
                    *score *= factor;
                } else {
                    *score /= factor;
                }
            }
        }

        // Sort by combined score descending
        let mut results: Vec<_> = combined_scores.into_iter().collect();
        results.sort_by(|(_, a), (_, b)| b.total_cmp(a));

        results
            .into_iter()
            .take(limit)
            .map(|(doc_idx, score)| (self.ids[doc_idx].clone(), score))
            .collect()
    }

    /// Get the number of unique terms in the index
    pub(crate) fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Get the number of documents in the index
    pub(crate) const fn document_count(&self) -> usize {
        self.ids.len()
    }
}

/// Location information for a documentation item.
#[derive(Debug, Clone)]
pub(crate) struct ItemLocation {
    pub crate_name: CrateName,
    pub item_path: Vec<u32>,
}

/// A search match with item location and relevance ranking.
#[derive(Debug, Clone)]
pub(crate) struct SearchMatch {
    pub item: ItemLocation,
    pub rank: f32,
}

/// Detailed search result with full item information.
#[derive(Debug, Clone)]
pub(crate) struct DetailedSearchResult {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub crate_name: Option<CrateName>,
    pub docs: Option<String>,
    pub id: Option<rustdoc_types::Id>,
    pub relevance: u32,
    pub source_crate: Option<CrateName>,
}

/// A search index for a specific crate.
pub(crate) struct TermIndex {
    crate_name: CrateName,
    terms: InvertedIndex,
}

impl TermIndex {
    /// Loads a cached search index, building one only when the cache cannot serve it.
    ///
    /// Invalidated when the rustdoc JSON is newer than the index, or the index came
    /// from an incompatible version. Building ahead of the cache check would cost
    /// what the cache exists to save, so it stays in the miss branch.
    pub(crate) fn load_or_build<'a>(
        request: &'a super::query::QueryContext,
        crate_name: &str,
    ) -> Result<Self, Vec<super::query::PathSuggestion<'a>>> {
        let mut suggestions = vec![];

        // Use QueryContext::resolve_path for crate validation
        let item = request
            .resolve_path(crate_name, &mut suggestions)
            .ok_or(suggestions)?;

        let crate_name = CrateName::new_unchecked(item.crate_index().name());

        // Preloaded crates keep source in a read-only sysroot, cache in a writable dir.
        let doc_path = request.doc_source_path(crate_name.as_str());
        let index_path = request.index_cache_path(crate_name.as_str());

        // block_on permits the non-Send ItemRef this future holds.
        Ok(tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let digest = source_digest(&doc_path);

                if let Some(terms) = Self::load(&index_path, digest).await {
                    metrics::record_load();
                    tracing::debug!(
                        crate_name = %crate_name,
                        terms = terms.terms.len(),
                        docs = terms.ids.len(),
                        "Loaded cached search index"
                    );
                    return Self { crate_name, terms };
                }

                let start = std::time::Instant::now();
                tracing::info!(crate_name = %crate_name, "Building search index");
                metrics::record_build();
                let terms = build_index(item);
                tracing::debug!(crate_name = %crate_name, elapsed = ?start.elapsed(), "Index build completed");

                if let Some(digest) = digest {
                    Self::store(&terms, &index_path, digest).await;
                }
                Self { crate_name, terms }
            })
        }))
    }

    /// Searches within this index and returns matches with location and rank.
    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<SearchMatch> {
        self.terms
            .search(query, limit)
            .into_iter()
            .map(|(item_path, rank)| SearchMatch {
                item: ItemLocation {
                    crate_name: self.crate_name.clone(),
                    item_path,
                },
                rank,
            })
            .collect()
    }

    /// Load a cached index, if it was built from this exact source content.
    async fn load(path: &Path, expected_digest: Option<u64>) -> Option<InvertedIndex> {
        let expected = index_header(expected_digest?);
        let owned = path.to_path_buf();

        // Deserialize in spawn_blocking since it's CPU intensive
        let loaded = tokio::task::spawn_blocking(move || {
            let mut file = std::fs::File::open(&owned).ok()?;

            let mut header = [0u8; INDEX_HEADER_LEN];
            if std::io::Read::read_exact(&mut file, &mut header).is_err() || header != expected {
                tracing::info!(
                    path = %owned.display(),
                    "Cached index does not match its source, discarding"
                );
                return None;
            }

            let mut buf = [0u8; 8192];
            if let Ok((terms, _)) = from_io((&mut file, &mut buf)) {
                tracing::debug!(path = %owned.display(), "Using cached index");
                return Some(terms);
            }
            tracing::warn!(path = %owned.display(), "Failed to deserialize cached index");
            None
        })
        .await
        .ok()
        .flatten();

        // Left in place a rejected index is re-read on every query.
        if loaded.is_none() {
            let _ = tokio::fs::remove_file(path).await;
        }
        loaded
    }

    /// Store an index to disk, stamped with the digest it was built from.
    async fn store(terms: &InvertedIndex, path: &Path, source_digest: u64) {
        let path = path.to_path_buf();
        let terms = terms.clone();

        // Serialize in spawn_blocking since it's CPU intensive
        tokio::task::spawn_blocking(move || {
            // Ensure the parent directory exists. This matters for preloaded
            // crates (e.g., stdlib) whose cache lives under a per-version
            // subdirectory that may not have been created yet.
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(
                    path = %path.display(),
                    error = ?e,
                    "Failed to create parent directory for index cache"
                );
                return;
            }

            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let written = std::io::Write::write_all(&mut file, &index_header(source_digest))
                        .map_err(|e| e.to_string())
                        .and_then(|()| to_io(&terms, &mut file).map(|_| ()).map_err(|e| e.to_string()));

                    if let Err(e) = written {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to write search index");
                        let _ = std::fs::remove_file(&path);
                    } else {
                        tracing::debug!(path = %path.display(), "Cached search index");
                    }
                }
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                    tracing::warn!(path = %path.display(), error = ?e, "Failed to create index file");
                }
                _ => {
                    // Already exists, another process may have created it
                    tracing::debug!(path = %path.display(), "Index file already exists");
                }
            }
        })
        .await
        .expect("Index storing task panicked");
    }
}

/// Builds an inverted index from a crate's documentation tree.
fn build_index(root_item: ItemRef<'_, Item>) -> InvertedIndex {
    let mut builder = TermBuilder::default();
    builder.recurse(root_item, &[], false);
    builder.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;

    /// Build a minimal InvertedIndex directly from (token, doc_idx, score) triples.
    /// Useful for testing scoring behavior without a real crate loaded.
    fn make_index(entries: Vec<(&str, usize, f32)>, doc_count: usize) -> InvertedIndex {
        let mut terms: HashMap<TermHash, Vec<(usize, f32)>> = HashMap::new();
        for (token, doc_idx, score) in entries {
            terms
                .entry(hash_term(token))
                .or_default()
                .push((doc_idx, score));
        }
        // Sort each bucket by score descending (as the real index does)
        for bucket in terms.values_mut() {
            bucket.sort_by(|(_, a), (_, b)| b.total_cmp(a));
        }
        // IDs: each doc gets a singleton path [doc_idx as u32]
        let ids: Vec<Vec<u32>> = (0..doc_count)
            .map(|i| vec![u32::try_from(i).expect("test doc_count fits in u32")])
            .collect();
        InvertedIndex::new(terms, ids)
    }

    /// Stem a single word using the same stemmer the index uses.
    fn stem(word: &str) -> String {
        let stemmer = Stemmer::create(Algorithm::English);
        tokenize_and_stem(word, &stemmer)
            .into_iter()
            .next()
            .unwrap_or_else(|| word.to_string())
    }

    /// When a multi-word query is issued, a document matching ALL query tokens should
    /// rank above one that matches only a subset, even if the partial-match document has
    /// a higher raw TF-IDF score for its single matching token.
    ///
    /// Without a coverage penalty, "cache invalidation" can surface items named
    /// "InvalidCharacter" (which match only the "invalid" stem with a high score) above
    /// the actual cache module (which matches both "cach" and "invalid" with lower scores).
    #[test]
    fn full_match_ranks_above_partial_match() {
        let cach = stem("cache");
        let invalid = stem("invalidation");

        // Doc 0: "cache_invalidation" — matches both stems (full match, low raw scores)
        // Doc 1: "invalid_char"       — matches only "invalid" with a much higher raw score
        let index = make_index(
            vec![
                (&cach, 0, 0.5),    // doc 0 contributes "cach"
                (&invalid, 0, 0.5), // doc 0 contributes "invalid"
                (&invalid, 1, 2.0), // doc 1 contributes "invalid" with 4x the score
            ],
            2,
        );

        let results = index.search("cache invalidation", 10);
        check!(!results.is_empty(), "Should return at least one result");
        check!(
            results[0].0 == vec![0u32],
            "Full match (doc 0) should rank above partial match (doc 1), \
             but top result was doc {:?}",
            results[0].0
        );
    }

    /// A document matching only part of a multi-word query must still surface with a
    /// usable score, ranked below the full match.
    ///
    /// An unfloored `(matched / total)^2` factor shrinks as `1/N^2` in the query length,
    /// leaving a single-term match on a three-word query at 11% of its score and pushing
    /// real matches past the result limit.
    #[test]
    fn partial_match_keeps_usable_score() {
        let cach = stem("cache");
        let index_term = stem("index");
        let invalid = stem("invalidation");

        // Doc 0 matches all three query terms, doc 1 matches only "index".
        let index = make_index(
            vec![
                (&cach, 0, 0.5),
                (&index_term, 0, 0.5),
                (&invalid, 0, 0.5),
                (&index_term, 1, 1.0),
            ],
            2,
        );

        let results = index.search("cache index invalidation", 10);
        check!(results.len() == 2, "Both documents should be returned");
        check!(results[0].0 == vec![0u32], "Full match should rank first");
        check!(results[1].0 == vec![1u32]);
        check!(
            results[1].1 >= COVERAGE_FLOOR,
            "Partial match kept only {} of its 1.0 raw score",
            results[1].1
        );
    }

    /// TF-IDF scores are negative for terms that are frequent relative to document
    /// length, and a multiplicative coverage penalty raises a negative score instead of
    /// lowering it. The full match must still win.
    #[test]
    fn coverage_factor_demotes_negative_scores() {
        let cach = stem("cache");
        let invalid = stem("invalidation");

        // Doc 0 matches both terms, doc 1 only one; both aggregate to a negative score.
        let index = make_index(
            vec![(&cach, 0, -0.5), (&invalid, 0, -0.5), (&invalid, 1, -0.9)],
            2,
        );

        let results = index.search("cache invalidation", 10);
        check!(results.len() == 2);
        check!(
            results[0].0 == vec![0u32],
            "Full match should rank above partial match, but top result was doc {:?}",
            results[0].0
        );
    }

    /// A headerless index decodes cleanly but matches nothing, so it must be rejected.
    #[tokio::test]
    async fn legacy_cache_without_version_header_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");

        let source = dir.path().join("demo.json");
        std::fs::write(&source, b"{}").expect("write source");

        // What an older binary left behind: bare postcard, no header.
        let index_path = dir.path().join("demo.index");
        let legacy = make_index(vec![("alpha", 0, 1.0)], 1);
        let mut file = std::fs::File::create(&index_path).expect("create index");
        to_io(&legacy, &mut file).expect("write legacy index");
        drop(file);

        let loaded = TermIndex::load(&index_path, source_digest(&source)).await;

        check!(loaded.is_none());
        check!(!index_path.exists());
    }

    /// A regenerated JSON can land just before an index built from the previous
    /// content, leaving the stale index newer than its source. mtime cannot catch
    /// that; the digest of what the index was built from can.
    #[tokio::test]
    async fn cache_built_from_different_source_content_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("demo.json");
        let index_path = dir.path().join("demo.index");

        let built_from = xxhash_rust::xxh3::xxh3_64(b"{\"old\":true}");

        // Source is regenerated first, so the index that follows is the newer file.
        std::fs::write(&source, b"{\"new\":true}").expect("write source");
        let stale = make_index(vec![("alpha", 0, 1.0)], 1);
        TermIndex::store(&stale, &index_path, built_from).await;

        let loaded = TermIndex::load(&index_path, source_digest(&source)).await;

        check!(loaded.is_none());
        check!(!index_path.exists());
    }

    /// The header must not break the case it guards: this build reads its own index.
    #[tokio::test]
    async fn cache_written_by_current_version_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");

        let source = dir.path().join("demo.json");
        std::fs::write(&source, b"{}").expect("write source");
        let digest = source_digest(&source);

        let index_path = dir.path().join("demo.index");
        let original = make_index(vec![("alpha", 0, 1.0)], 1);
        TermIndex::store(&original, &index_path, digest.expect("digest")).await;

        let loaded = TermIndex::load(&index_path, digest).await;

        let loaded = loaded.expect("index should round-trip");
        check!(loaded.search(&stem("alpha"), 5).len() == 1);
        check!(index_path.exists());
    }
}
