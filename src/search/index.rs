//! TF-IDF inverted index implementation for full-text search.

use crate::item::ItemRef;
use crate::types::CrateName;
use postcard::{from_io, to_io};
use rustdoc_types::Item;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, time::SystemTime};

use super::tokenize::{TermBuilder, hash_term, tokenize_and_stem};
use rust_stemmers::{Algorithm, Stemmer};

/// Term hash for fast lookup
type TermHash = u64;

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
    pub(super) fn new(terms: HashMap<TermHash, Vec<(usize, f32)>>, ids: Vec<Vec<u32>>) -> Self {
        Self { terms, ids }
    }

    /// Searches for items matching the query term using TF-IDF scoring.
    /// Returns item ID paths sorted by relevance score (highest first).
    ///
    /// The query is tokenized and stemmed just like indexed terms, so:
    /// - "BackgroundWorker" matches items with "background", "worker", or "backgroundwork"
    /// - CamelCase, snake_case, and hyphen-case are all handled
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

        // Apply a quadratic coverage penalty only for multi-word (space-separated) queries.
        // When the user types "cache invalidation", items that only match "invalid" (not
        // "cache") should rank below items matching both words. The penalty is (matched/total)^2,
        // so a document matching 1 of 2 words gets a 0.25× multiplier.
        //
        // We do NOT apply this penalty for single-identifier queries like "TypeFormatter" or
        // path queries like "serde::Serialize" — those produce multiple tokens via CamelCase
        // splitting, but every token comes from the same user-supplied name, so partial matches
        // are still meaningful and penalizing them would cause real items to drop out of results.
        let total_tokens = tokens.len() as f32;
        if query.contains(' ') && total_tokens > 1.0 {
            for (doc_idx, score) in combined_scores.iter_mut() {
                let matched = token_match_counts.get(doc_idx).copied().unwrap_or(0) as f32;
                let coverage = matched / total_tokens;
                *score *= coverage * coverage;
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
    pub(crate) fn document_count(&self) -> usize {
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
    /// Prepares index data synchronously, resolving crate and building the index.
    /// Returns data needed for async cache operations.
    ///
    /// This is split from `load_or_build_async` to avoid holding non-Send QueryContext
    /// references in async functions.
    fn prepare_index<'a>(
        request: &'a super::query::QueryContext,
        crate_name: &str,
    ) -> Result<
        (
            CrateName,
            std::path::PathBuf,
            std::path::PathBuf,
            InvertedIndex,
        ),
        Vec<super::query::PathSuggestion<'a>>,
    > {
        let mut suggestions = vec![];

        // Use QueryContext::resolve_path for crate validation
        let item = request
            .resolve_path(crate_name, &mut suggestions)
            .ok_or(suggestions)?;

        let crate_index = item.crate_index();
        let crate_name = CrateName::new_unchecked(crate_index.name());

        // Get paths for docs and index
        let target_doc = request.workspace_root().join("target/doc");
        let doc_path = crate_name.doc_json_path(&target_doc);
        let index_path = crate_name.index_path(&target_doc);

        // Build index synchronously
        let start = std::time::Instant::now();
        tracing::info!(crate_name = %crate_name, "Building search index");
        let terms = build_index(item);
        tracing::debug!(crate_name = %crate_name, elapsed = ?start.elapsed(), "Index build completed");

        Ok((crate_name, doc_path, index_path, terms))
    }

    /// Async portion: checks cache and stores/returns the index.
    async fn load_or_build_async(
        crate_name: CrateName,
        doc_path: std::path::PathBuf,
        index_path: std::path::PathBuf,
        prepared_terms: InvertedIndex,
    ) -> Self {
        // Get mtime of rustdoc JSON
        let mtime = tokio::fs::metadata(&doc_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());

        // Try loading cached index
        if let Some(terms) = Self::load(&index_path, mtime).await {
            tracing::debug!(
                crate_name = %crate_name,
                terms = terms.terms.len(),
                docs = terms.ids.len(),
                "Loaded cached search index"
            );
            return Self { crate_name, terms };
        }

        // Use the pre-built index
        Self::store(&prepared_terms, &index_path).await;
        Self {
            terms: prepared_terms,
            crate_name,
        }
    }

    /// Loads a cached search index or builds a new one if cache is stale.
    /// Cache is invalidated when rustdoc JSON is newer than the index file.
    ///
    /// This is a blocking wrapper around async cache operations to avoid
    /// Send/Sync issues with QueryContext in async functions.
    pub(crate) fn load_or_build<'a>(
        request: &'a super::query::QueryContext,
        crate_name: &str,
    ) -> Result<Self, Vec<super::query::PathSuggestion<'a>>> {
        // Synchronous: resolve crate and build index
        let (crate_name, doc_path, index_path, terms) = Self::prepare_index(request, crate_name)?;

        // Use tokio::task::block_in_place to allow blocking within an async runtime
        // This works whether called from sync or async context
        Ok(tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(Self::load_or_build_async(
                crate_name, doc_path, index_path, terms,
            ))
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

    /// Load a cached index from disk.
    async fn load(path: &Path, mtime: Option<SystemTime>) -> Option<InvertedIndex> {
        let file = tokio::fs::File::open(path).await.ok()?;
        let index_mtime = file.metadata().await.ok()?.modified().ok()?;

        let mtime = mtime?;
        // Check if index is NEWER than source docs
        if index_mtime.duration_since(mtime).is_ok() {
            let path = path.to_path_buf();
            // Deserialize in spawn_blocking since it's CPU intensive
            tokio::task::spawn_blocking(move || {
                let mut file = std::fs::File::open(&path).ok()?;
                let mut buf = [0u8; 8192];
                if let Ok((terms, _)) = from_io((&mut file, &mut buf)) {
                    tracing::debug!(path = %path.display(), "Using cached index (newer than source)");
                    return Some(terms);
                } else {
                    tracing::warn!(path = %path.display(), "Failed to deserialize cached index");
                }
                None
            })
            .await
            .ok()?
        } else {
            // Delete stale index
            tracing::info!(path = %path.display(), "Cache stale or invalid, will rebuild index");
            let _ = tokio::fs::remove_file(path).await;
            None
        }
    }

    /// Store an index to disk.
    async fn store(terms: &InvertedIndex, path: &Path) {
        let path = path.to_path_buf();
        let terms = terms.clone();

        // Serialize in spawn_blocking since it's CPU intensive
        tokio::task::spawn_blocking(move || {
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(mut file) => {
                    if let Err(e) = to_io(&terms, &mut file) {
                        tracing::warn!(path = %path.display(), error = ?e, "Failed to write search index");
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
        let ids: Vec<Vec<u32>> = (0..doc_count).map(|i| vec![i as u32]).collect();
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
}
