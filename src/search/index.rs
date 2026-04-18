use super::{tokenize::tokenize, xor::Xor8Filter};

/// A single document's entry in a search index.
///
/// Associates a document name (skill name, built-in command name) with
/// the XOR filter built from its tokenized content.
#[derive(Debug)]
pub(crate) struct IndexEntry {
    /// The document identifier (e.g., "deploy rollback", "creft add").
    pub name: String,
    /// Short description for display in search results.
    pub description: String,
    /// The XOR filter built from the document's tokenized content.
    pub filter: Xor8Filter,
}

/// A collection of document entries searchable by token membership.
///
/// The index is immutable after construction. To update, rebuild the
/// entire index for the namespace.
#[derive(Debug)]
pub(crate) struct SearchIndex {
    entries: Vec<IndexEntry>,
}

impl SearchIndex {
    /// Build an index from a list of `(name, description, text)` tuples.
    ///
    /// Each text is tokenized and an XOR filter is built from the tokens.
    /// Documents with no tokens (empty text) are included with an empty
    /// filter that rejects all queries.
    pub fn build(documents: &[(&str, &str, &str)]) -> Self {
        let entries = documents
            .iter()
            .map(|&(name, description, text)| {
                let tokens = tokenize(text);
                IndexEntry {
                    name: name.to_owned(),
                    description: description.to_owned(),
                    filter: Xor8Filter::build(&tokens),
                }
            })
            .collect();
        Self { entries }
    }

    /// Query the index for documents that might contain all query tokens.
    ///
    /// Tokenizes the query, then tests each document's filter for membership
    /// of every query token. Returns entries where all tokens are probably
    /// present (AND semantics). False positives are possible (~0.39% per
    /// token per document).
    ///
    /// An empty query returns all entries.
    pub fn search(&self, query: &str) -> Vec<&IndexEntry> {
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return self.entries.iter().collect();
        }
        self.entries
            .iter()
            .filter(|entry| tokens.iter().all(|&tok| entry.filter.contains(tok)))
            .collect()
    }

    /// Serialize the index to bytes.
    ///
    /// Format:
    /// `[entry_count: u32 LE]`
    /// For each entry:
    ///   `[name_len: u16 LE][name: UTF-8 bytes]`
    ///   `[desc_len: u16 LE][desc: UTF-8 bytes]`
    ///   `[filter_len: u32 LE][filter: bytes from Xor8Filter::to_bytes()]`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            let desc_bytes = entry.description.as_bytes();
            let filter_bytes = entry.filter.to_bytes();

            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(&(desc_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(desc_bytes);
            out.extend_from_slice(&(filter_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&filter_bytes);
        }
        out
    }

    /// Deserialize an index from bytes.
    ///
    /// Returns `None` if the data is malformed or truncated.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut pos = 0;

        let entry_count = read_u32(data, &mut pos)? as usize;
        // Cap the pre-allocation to avoid OOM on a corrupt entry_count. The loop's
        // `?` propagation still bounds actual entries pushed to the real count.
        let mut entries = Vec::with_capacity(entry_count.min(4096));

        for _ in 0..entry_count {
            let name_len = read_u16(data, &mut pos)? as usize;
            let name = read_str(data, &mut pos, name_len)?;

            let desc_len = read_u16(data, &mut pos)? as usize;
            let description = read_str(data, &mut pos, desc_len)?;

            let filter_len = read_u32(data, &mut pos)? as usize;
            let filter_bytes = read_bytes(data, &mut pos, filter_len)?;
            let filter = Xor8Filter::from_bytes(filter_bytes)?;

            entries.push(IndexEntry {
                name,
                description,
                filter,
            });
        }

        // Reject trailing bytes — malformed or appended garbage.
        if pos != data.len() {
            return None;
        }

        Some(Self { entries })
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── deserialization helpers ───────────────────────────────────────────────────

fn read_u32(data: &[u8], pos: &mut usize) -> Option<u32> {
    let bytes = data.get(*pos..*pos + 4)?;
    *pos += 4;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u16(data: &[u8], pos: &mut usize) -> Option<u16> {
    let bytes = data.get(*pos..*pos + 2)?;
    *pos += 2;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn read_bytes<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Option<&'a [u8]> {
    let bytes = data.get(*pos..*pos + len)?;
    *pos += len;
    Some(bytes)
}

fn read_str(data: &[u8], pos: &mut usize, len: usize) -> Option<String> {
    let bytes = read_bytes(data, pos, len)?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn build_three_doc_index() -> SearchIndex {
        SearchIndex::build(&[
            (
                "deploy rollback",
                "Roll back a deployment",
                "rollback procedure steps template",
            ),
            (
                "deploy push",
                "Push a build to an environment",
                "push build artifact to environment",
            ),
            (
                "aws copy",
                "Copy S3 objects",
                "copy objects between buckets placeholder",
            ),
        ])
    }

    // ── build + search ────────────────────────────────────────────────────────

    #[test]
    fn empty_query_returns_all_entries() {
        let idx = build_three_doc_index();
        let results = idx.search("");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn single_token_query_returns_matching_documents() {
        let idx = build_three_doc_index();
        let results = idx.search("template");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "deploy rollback");
    }

    #[test]
    fn multi_token_query_and_semantics() {
        let idx = build_three_doc_index();
        // "template" is only in "deploy rollback"; "placeholder" is only in "aws copy"
        let results = idx.search("template placeholder");
        assert_eq!(
            results.len(),
            0,
            "AND semantics: no doc contains both tokens"
        );
    }

    #[test]
    fn multi_token_query_matching_single_document() {
        let idx = build_three_doc_index();
        let results = idx.search("template rollback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "deploy rollback");
    }

    #[test]
    fn empty_index_returns_no_results() {
        let idx = SearchIndex::build(&[]);
        assert!(idx.search("anything").is_empty());
        assert!(idx.search("").is_empty());
    }

    #[test]
    fn document_with_no_tokens_excluded_from_token_queries() {
        let idx = SearchIndex::build(&[
            ("empty doc", "An empty document", ""),
            ("real doc", "A real document", "rollback procedure"),
        ]);
        // Empty query returns all
        assert_eq!(idx.search("").len(), 2);
        // Token query should NOT return the empty doc
        let results = idx.search("rollback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "real doc");
    }

    // ── serialization round-trip ──────────────────────────────────────────────

    #[test]
    fn round_trip_preserves_entry_count() {
        let idx = build_three_doc_index();
        let bytes = idx.to_bytes();
        let restored = SearchIndex::from_bytes(&bytes).expect("round-trip should succeed");
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn round_trip_preserves_search_behavior() {
        let idx = build_three_doc_index();
        let bytes = idx.to_bytes();
        let restored = SearchIndex::from_bytes(&bytes).expect("round-trip should succeed");

        let original_results: Vec<&str> = idx
            .search("template")
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        let restored_results: Vec<&str> = restored
            .search("template")
            .iter()
            .map(|e| e.name.as_str())
            .collect();

        assert_eq!(original_results, restored_results);
    }

    #[test]
    fn round_trip_preserves_names_and_descriptions() {
        let idx = SearchIndex::build(&[("my skill", "A short description", "some content here")]);
        let bytes = idx.to_bytes();
        let restored = SearchIndex::from_bytes(&bytes).expect("round-trip should succeed");
        assert_eq!(restored.entries[0].name, "my skill");
        assert_eq!(restored.entries[0].description, "A short description");
    }

    #[test]
    fn empty_index_round_trips_correctly() {
        let idx = SearchIndex::build(&[]);
        let bytes = idx.to_bytes();
        let restored =
            SearchIndex::from_bytes(&bytes).expect("empty index round-trip should succeed");
        assert!(restored.is_empty());
        assert!(restored.search("anything").is_empty());
    }

    #[test]
    fn from_bytes_rejects_empty_slice() {
        assert!(SearchIndex::from_bytes(&[]).is_none());
    }

    #[test]
    fn from_bytes_rejects_truncated_data() {
        let idx = build_three_doc_index();
        let bytes = idx.to_bytes();
        // Truncate partway through
        let truncated = &bytes[..bytes.len() / 2];
        assert!(SearchIndex::from_bytes(truncated).is_none());
    }

    #[test]
    fn from_bytes_rejects_trailing_garbage() {
        let idx = SearchIndex::build(&[("doc", "desc", "content")]);
        let mut bytes = idx.to_bytes();
        bytes.push(0xFF);
        assert!(SearchIndex::from_bytes(&bytes).is_none());
    }

    // ── len / is_empty ────────────────────────────────────────────────────────

    #[test]
    fn len_and_is_empty_reflect_entry_count() {
        let empty = SearchIndex::build(&[]);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());

        let one = SearchIndex::build(&[("doc", "desc", "content")]);
        assert_eq!(one.len(), 1);
        assert!(!one.is_empty());
    }
}
