//! Text tokenization and stemming utilities for search indexing.

use crate::item::ItemRef;
use ahash::{AHashMap, AHasher};
use rust_stemmers::{Algorithm, Stemmer};
use rustdoc_types::{Item, ItemEnum};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use super::index::InvertedIndex;

/// Minimum token length for indexing. Set to 1 to allow short Rust types like `u8`, `i32`, `io`.
const MIN_TOKEN_LENGTH: usize = 1;

/// Common English stop words to filter out from indexing.
/// These high-frequency words add little value to search relevance.
pub(crate) const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "he", "in", "is", "it",
    "its", "of", "on", "that", "the", "to", "was", "will", "with",
];

/// Document identifier: (crate_id, item_id)
type DocId = (u64, u32);

/// Term hash for fast lookup
type TermHash = u64;

/// Builder for accumulating term frequencies before TF-IDF finalization.
pub(crate) struct TermBuilder {
    /// Flat map from (term_hash, doc_id) → raw TF score
    term_docs: HashMap<(TermHash, DocId), f32>,
    /// Map from doc_id to id_path (sequence of u32 IDs from crate root to item)
    shortest_paths: HashMap<DocId, Vec<u32>>,
    /// Map from doc_id to document length (total term count for normalization)
    doc_lengths: HashMap<DocId, usize>,
    /// Reusable stemmer instance for English language stemming
    stemmer: Stemmer,
}

impl Default for TermBuilder {
    fn default() -> Self {
        Self {
            term_docs: HashMap::default(),
            shortest_paths: HashMap::default(),
            doc_lengths: HashMap::default(),
            stemmer: Stemmer::create(Algorithm::English),
        }
    }
}

impl TermBuilder {
    /// Add a term with its TF score for a specific document.
    fn add(&mut self, term: &str, tf_score: f32, doc_id: DocId) {
        let term_hash = hash_term(term);
        *self.term_docs.entry((term_hash, doc_id)).or_insert(0.0) += tf_score;
    }

    /// Extracts and adds terms from text with frequency counting.
    /// TF score = term_count * base_score, where base_score weights importance (e.g., 2.0 for names, 1.0 for docs).
    fn add_terms(&mut self, text: &str, doc_id: DocId, base_score: f32) {
        let words = tokenize_and_stem(text, &self.stemmer);

        // Count word frequencies using AHashMap for O(1) operations
        let mut word_counts: AHashMap<String, usize> = AHashMap::with_capacity(words.len());
        for word in words {
            *word_counts.entry(word).or_insert(0) += 1;
        }

        // Track document length for normalization
        let doc_len: usize = word_counts.values().sum();
        *self.doc_lengths.entry(doc_id).or_insert(0) += doc_len;

        // TF = count * base_score
        for (word, count) in word_counts {
            let tf_score = (count as f32) * base_score;
            self.add(&word, tf_score, doc_id);
        }
    }

    /// Calculates IDF scores and produces the final searchable index.
    /// Uses formula: TF-IDF = (1 + ln(tf_normalized)) * ln(total_docs / doc_freq),
    /// where tf_normalized = tf / doc_length for length normalization.
    pub(crate) fn finalize(self) -> InvertedIndex {
        let start = std::time::Instant::now();
        let total_docs = self.shortest_paths.len() as f32;

        // Calculate average document length for normalization
        let total_length: usize = self.doc_lengths.values().sum();
        let avg_doc_length = if !self.doc_lengths.is_empty() {
            total_length as f32 / self.doc_lengths.len() as f32
        } else {
            1.0
        };

        // Sort shortest_paths by doc_id for deterministic output
        let mut sorted_paths: Vec<_> = self.shortest_paths.into_iter().collect();
        sorted_paths.sort_by_key(|(doc_id, _)| *doc_id);

        // Build id_set mapping from doc_id to array index
        let mut id_set: HashMap<DocId, usize> = HashMap::new();
        let mut ids: Vec<Vec<u32>> = Vec::new();

        for (doc_id, path) in sorted_paths {
            let index = ids.len();
            ids.push(path);
            id_set.insert(doc_id, index);
        }

        // Group flat term_docs by term_hash
        type GroupedDocs = HashMap<TermHash, Vec<(DocId, f32)>>;
        let mut grouped: GroupedDocs = HashMap::new();
        let total_term_doc_pairs = self.term_docs.len(); // Capture before move
        for ((term_hash, doc_id), tf_score) in self.term_docs {
            grouped
                .entry(term_hash)
                .or_default()
                .push((doc_id, tf_score));
        }

        // Calculate TF-IDF scores
        let mut terms: HashMap<TermHash, Vec<(usize, f32)>> = HashMap::new();

        for (term_hash, doc_scores) in grouped {
            // IDF = ln(total_docs / doc_freq)
            let doc_freq = doc_scores.len() as f32;
            let idf = (total_docs / doc_freq).ln();

            // TF-IDF with length normalization
            let mut tf_idf_scores: Vec<_> = doc_scores
                .into_iter()
                .filter_map(|(doc_id, tf_score)| {
                    let doc_length = self.doc_lengths.get(&doc_id).copied().unwrap_or(1) as f32;
                    // Normalize TF by document length relative to average
                    let length_norm = doc_length / avg_doc_length;
                    let tf_normalized = tf_score / length_norm.max(0.5); // Clamp to prevent over-penalization

                    id_set
                        .get(&doc_id)
                        .map(|&idx| (idx, (1.0 + tf_normalized.ln()) * idf))
                })
                .collect();

            // Sort descending by score
            tf_idf_scores.sort_by(|(_, a), (_, b)| b.total_cmp(a));

            terms.insert(term_hash, tf_idf_scores);
        }

        let index = InvertedIndex::new(terms, ids);

        tracing::info!(
            "Built search index: {} unique terms, {} documents, {} term-document pairs in {:?}",
            index.term_count(),
            index.document_count(),
            total_term_doc_pairs,
            start.elapsed()
        );

        index
    }

    /// Recursively index an item and its children.
    pub(crate) fn recurse(&mut self, item: ItemRef<'_, Item>, path: &[u32], track_path: bool) {
        let id_num = item.id.0;

        let mut new_path = path.to_vec();
        if track_path {
            new_path.push(id_num);
        }

        // Create document ID (crate_id, item_id)
        let crate_id = item.crate_index().root().0 as u64;
        let doc_id = (crate_id, id_num);

        // Track shortest path to this item
        if track_path {
            self.shortest_paths
                .entry(doc_id)
                .or_insert_with(|| new_path.clone());
        }

        // Index name with higher weight (base_score: 2.0)
        if let Some(name) = item.name() {
            self.add_terms(name, doc_id, 2.0);
        }

        // Index documentation with lower weight (base_score: 1.0)
        if let Some(docs) = item.comment() {
            self.add_terms(docs, doc_id, 1.0);
        }

        // Recurse into children
        match item.inner() {
            ItemEnum::Module(_) | ItemEnum::Enum(_) => {
                // Use include_use() to also get re-exports as Use items
                for child in item.children().include_use().build() {
                    if let ItemEnum::Use(use_item) = child.inner() {
                        // Index re-exports under their public name
                        self.index_reexport(child, use_item, &new_path);
                    } else {
                        self.recurse(child, &new_path, true);
                    }
                }
            }
            ItemEnum::Struct(_) | ItemEnum::Union(_) | ItemEnum::Trait(_) => {
                // Index methods but don't include in path
                for method in item.methods() {
                    self.recurse(method, &new_path, false);
                }
            }
            _ => {}
        }
    }

    /// Index a re-export item under its public name.
    ///
    /// This ensures that `pub use other::Thing` makes `Thing` searchable
    /// under the re-exporting module's namespace.
    fn index_reexport(
        &mut self,
        use_ref: ItemRef<'_, Item>,
        use_item: &rustdoc_types::Use,
        path: &[u32],
    ) {
        // Skip glob imports - they're expanded by the iterator
        if use_item.is_glob {
            return;
        }

        let id_num = use_ref.id.0;
        let crate_id = use_ref.crate_index().root().0 as u64;
        let doc_id = (crate_id, id_num);

        // Create path including this re-export
        let mut reexport_path = path.to_vec();
        reexport_path.push(id_num);

        // Track path to this re-export
        self.shortest_paths
            .entry(doc_id)
            .or_insert_with(|| reexport_path);

        // Index the re-export name (e.g., "Serialize" from `pub use serde_core::Serialize`)
        self.add_terms(&use_item.name, doc_id, 2.0);

        // Try to resolve the target to get its documentation
        let target = use_item
            .id
            .and_then(|id| use_ref.get(&id))
            .or_else(|| use_ref.query().resolve_path(&use_item.source, &mut vec![]));

        if let Some(target_item) = target {
            // Index target's documentation under the re-export's identity
            if let Some(docs) = target_item.comment() {
                self.add_terms(docs, doc_id, 1.0);
            }
        }
    }
}

/// Tokenizes text into searchable terms with stemming and case-aware splitting.
///
/// This function implements a state machine that splits text on multiple boundaries:
/// - **CamelCase**: "HttpServer" → ["Http", "Server", "HttpServer"]
/// - **snake_case**: "parse_json" → ["parse", "json"]
/// - **hyphen-case**: "multi-line" → ["multi", "line"]
///
/// The state machine maintains two pointers:
/// - `word_start`: Start of the complete word (e.g., "HttpServer")
/// - `subword_start`: Start of the current sub-component (e.g., "Server")
///
/// This allows extracting both individual components and the full compound term.
pub(crate) fn tokenize_and_stem(text: &str, stemmer: &Stemmer) -> Vec<String> {
    let mut tokens = vec![];

    // State machine variables
    let mut last_case = None; // Track case transitions (None/Some(false)/Some(true))
    let mut word_start = 0; // Start of full word (e.g., "HttpServer")
    let mut subword_start = 0; // Start of subword (e.g., "Server")
    let mut word_start_next_char = true; // Flag: start new word at next char
    let mut subword_start_next_char = true; // Flag: start new subword at next char

    for (i, c) in text.char_indices() {
        // Initialize word/subword pointers at the start of a new word
        if word_start_next_char {
            word_start = i;
            subword_start = i;
            word_start_next_char = false;
            subword_start_next_char = false;
        }

        // Initialize subword pointer for CamelCase boundaries
        if subword_start_next_char {
            subword_start = i;
            subword_start_next_char = false;
        }

        // Detect case changes for CamelCase splitting (lowercase → uppercase)
        let current_case = c.is_alphabetic().then(|| c.is_uppercase());
        let case_change = last_case == Some(false) && current_case == Some(true);
        last_case = current_case;

        if c == '-' || c == '_' {
            // **Snake_case / hyphen-case boundary**: "parse_json" or "multi-line"
            // Extract the current subword (e.g., "parse" from "parse_json")
            if i.saturating_sub(subword_start) >= MIN_TOKEN_LENGTH {
                index_token(&text[subword_start..i], &mut tokens, stemmer);
            }
            // Start a new subword after the delimiter
            subword_start_next_char = true;
        } else if !c.is_alphabetic() {
            // **Non-alphabetic character**: End of complete word
            // Extract last subword if different from word start (e.g., "Server" from "HttpServer123")
            if i.saturating_sub(subword_start) >= MIN_TOKEN_LENGTH && subword_start != word_start {
                index_token(&text[subword_start..i], &mut tokens, stemmer);
            }
            // Extract complete word (e.g., "HttpServer" from "HttpServer123")
            if i.saturating_sub(word_start) >= MIN_TOKEN_LENGTH {
                index_token(&text[word_start..i], &mut tokens, stemmer);
            }
            // Start a new word after this non-alphabetic character
            word_start_next_char = true;
        } else if case_change {
            // **CamelCase boundary**: lowercase → uppercase (e.g., "http" → "S" in "httpServer")
            // Extract the previous subword (e.g., "http" before "Server")
            if i.saturating_sub(subword_start) >= MIN_TOKEN_LENGTH {
                index_token(&text[subword_start..i], &mut tokens, stemmer);
            }
            // Start new subword at the uppercase character
            subword_start = i;
        }
    }

    // **Handle final tokens** at end of string
    if !word_start_next_char {
        // Extract last subword if it's different from word start
        let last_subword = &text[subword_start..];
        if word_start != subword_start && last_subword.len() >= MIN_TOKEN_LENGTH {
            index_token(last_subword, &mut tokens, stemmer);
        }
        // Extract complete final word
        let last_word = &text[word_start..];
        if last_word.len() >= MIN_TOKEN_LENGTH {
            index_token(last_word, &mut tokens, stemmer);
        }
    }

    tokens
}

/// Add a token using proper stemming algorithm, filtering out stop words.
pub(crate) fn index_token(token: &str, tokens: &mut Vec<String>, stemmer: &Stemmer) {
    let lowercase = token.to_lowercase();

    // Skip stop words
    if STOP_WORDS.contains(&lowercase.as_str()) {
        return;
    }

    let stemmed = stemmer.stem(&lowercase);
    tokens.push(stemmed.into_owned());
}

/// Hashes a term for fast lookup (case-insensitive).
pub(crate) fn hash_term(term: &str) -> u64 {
    let mut hasher = AHasher::default();
    term.to_lowercase().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use rstest::rstest;

    #[rstest]
    #[case("CamelCase", &["camel", "case", "camelcas"])] // Now lowercase
    #[case("snake_case", &["snake", "case"])]
    #[case("hyphen-case", &["hyphen", "case"])]
    #[case("CamelCases hyphenate-words snake_words", &["camel", "case", "hyphen", "word", "snake"])] // Lowercase
    fn test_extract_tokens_contains(#[case] input: &str, #[case] expected_tokens: &[&str]) {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(input, &stemmer);
        for expected in expected_tokens {
            check!(tokens.contains(&expected.to_string()));
        }
    }

    #[rstest]
    #[case("plurals", vec!["plural"])]
    #[case("ab abc", vec!["ab", "abc"])] // "a" is a stop word, filtered out
    fn test_extract_tokens_exact(#[case] input: &str, #[case] expected: Vec<&str>) {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(input, &stemmer);
        let expected_owned: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        check!(tokens == expected_owned);
    }

    #[rstest]
    #[case("u8", vec!["u"])] // "8" is non-alphabetic and discarded
    #[case("i32", vec!["i"])] // "32" is non-alphabetic and discarded
    #[case("f64", vec!["f"])] // "64" is non-alphabetic and discarded
    #[case("io", vec!["io"])]
    fn test_short_rust_types_indexed(#[case] input: &str, #[case] expected: Vec<&str>) {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(input, &stemmer);
        let expected_owned: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        check!(tokens == expected_owned);
    }

    #[rstest]
    #[case("the quick brown fox", vec!["quick", "brown", "fox"])]
    #[case("a function for parsing", vec!["function", "pars"])] // "parsing" → "pars"
    #[case("is it working", vec!["work"])] // "working" → "work"
    fn test_stop_words_filtered(#[case] input: &str, #[case] expected_contains: Vec<&str>) {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(input, &stemmer);

        // Verify stop words are NOT in tokens
        for stop_word in STOP_WORDS {
            check!(!tokens.contains(&stop_word.to_string()));
        }

        // Verify expected tokens ARE in results
        for expected in expected_contains {
            check!(tokens.contains(&expected.to_string()));
        }
    }

    #[test]
    fn test_case_insensitive_hashing() {
        check!(hash_term("HashMap") == hash_term("hashmap"));
        check!(hash_term("HASHMAP") == hash_term("hashmap"));
        check!(hash_term("hashMap") == hash_term("HashMap"));
    }

    #[rstest]
    #[case("Vec2", &["vec"])] // "2" is non-alphabetic and discarded
    #[case("HTTP2Server", &["http", "server"])] // "2" splits the word
    fn test_tokenization_with_numbers(#[case] input: &str, #[case] expected_contains: &[&str]) {
        let stemmer = Stemmer::create(Algorithm::English);
        let tokens = tokenize_and_stem(input, &stemmer);
        for expected in expected_contains {
            check!(tokens.contains(&expected.to_string()));
        }
    }

    #[rstest]
    #[case("Москва")] // Cyrillic
    #[case("日本")] // Japanese
    #[case("🦀")] // Emoji
    fn test_unicode_handling(#[case] input: &str) {
        let stemmer = Stemmer::create(Algorithm::English);
        // Should not panic, even if it produces empty results
        let _tokens = tokenize_and_stem(input, &stemmer);
    }

    #[test]
    fn test_empty_and_whitespace() {
        let stemmer = Stemmer::create(Algorithm::English);
        check!(tokenize_and_stem("", &stemmer).is_empty());
        check!(tokenize_and_stem("   ", &stemmer).is_empty());
        check!(tokenize_and_stem("\n\t", &stemmer).is_empty());
    }
}
