//! TF-IDF inverted index implementation for full-text search.

use crate::item::ItemRef;
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
pub struct InvertedIndex {
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
    pub fn search(&self, query: &str, limit: usize) -> Vec<(Vec<u32>, f32)> {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(query, &stemmer);

        if tokens.is_empty() {
            return vec![];
        }

        // Collect results from all tokens, combining scores for documents that match multiple
        let mut combined_scores: HashMap<usize, f32> = HashMap::new();

        for token in &tokens {
            let term_hash = hash_term(token);
            if let Some(results) = self.terms.get(&term_hash) {
                for (doc_idx, score) in results {
                    *combined_scores.entry(*doc_idx).or_insert(0.0) += score;
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
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Get the number of documents in the index
    pub fn document_count(&self) -> usize {
        self.ids.len()
    }
}

/// Location information for a documentation item.
#[derive(Debug, Clone)]
pub struct ItemLocation {
    pub crate_name: String,
    pub item_path: Vec<u32>,
}

/// A search match with item location and relevance ranking.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub item: ItemLocation,
    pub rank: f32,
}

/// Detailed search result with full item information.
#[derive(Debug, Clone)]
pub struct DetailedSearchResult {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub crate_name: Option<String>,
    pub docs: Option<String>,
    pub id: Option<rustdoc_types::Id>,
    pub relevance: u32,
    pub source_crate: Option<String>,
}

/// A search index for a specific crate.
pub struct TermIndex {
    crate_name: String,
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
            String,
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
        let crate_name = crate_index.name().to_string();

        // Get paths for docs and index
        let doc_path = request
            .workspace_root()
            .join("target/doc")
            .join(format!("{}.json", crate_name.replace('-', "_")));

        let index_path = doc_path
            .parent()
            .unwrap()
            .join(format!("{}.index", crate_name.replace('-', "_")));

        // Build index synchronously
        let start = std::time::Instant::now();
        tracing::info!("Building search index for '{}'", crate_name);
        let terms = build_index(item);
        tracing::debug!("Index build completed in {:?}", start.elapsed());

        Ok((crate_name, doc_path, index_path, terms))
    }

    /// Async portion: checks cache and stores/returns the index.
    async fn load_or_build_async(
        crate_name: String,
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
                "Loaded cached search index for '{}' ({} terms, {} docs)",
                crate_name,
                terms.terms.len(),
                terms.ids.len()
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
    pub fn load_or_build<'a>(
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
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchMatch> {
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
                    tracing::debug!("Using cached index (newer than source)");
                    return Some(terms);
                } else {
                    tracing::warn!("Failed to deserialize cached index at {}", path.display());
                }
                None
            })
            .await
            .ok()?
        } else {
            // Delete stale index
            tracing::info!(
                "Cache stale or invalid, will rebuild index (file: {})",
                path.display()
            );
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
                        tracing::warn!("Failed to write search index to {}: {}", path.display(), e);
                        let _ = std::fs::remove_file(&path);
                    } else {
                        tracing::debug!("Cached search index to {}", path.display());
                    }
                }
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                    tracing::warn!("Failed to create index file {}: {}", path.display(), e);
                }
                _ => {
                    // Already exists, another process may have created it
                    tracing::debug!("Index file already exists at {}", path.display());
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
