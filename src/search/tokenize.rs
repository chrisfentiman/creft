use std::collections::HashSet;

/// Tokenize text into a set of 64-bit hashes.
///
/// Splits on whitespace and punctuation boundaries — any character that is
/// neither alphanumeric, `_`, nor `-` is a delimiter. Each resulting token is
/// lowercased, filtered to at least 2 characters, and hashed to a `u64` using
/// FNV-1a. Duplicate tokens are deduplicated; order is unspecified.
///
/// Examples:
/// - `"Hello World"` → 2 hashes
/// - `"hello,world"` → 2 hashes (`","` is a punctuation boundary)
/// - `"rollback-plan"` → 1 hash (hyphen kept within token)
/// - `"it's a test!"` → hashes for `"it"` and `"test"` (`"s"` and `"a"` are < 2 chars)
///
/// The hash function is FNV-1a (64-bit), separate from the SplitMix64 used
/// by the XOR filter internally. FNV-1a is fast for short strings and produces
/// good distribution for filter construction.
pub(crate) fn tokenize(text: &str) -> Vec<u64> {
    let mut hashes: Vec<u64> = split_and_lowercase(text)
        .map(|tok| hash_token(&tok))
        .collect();

    hashes.sort_unstable();
    hashes.dedup();
    hashes
}

/// Hash a single normalized token string to a u64.
///
/// Uses FNV-1a (64-bit) for deterministic, stable hashes across Rust versions.
/// Do NOT use `std::hash::Hasher` — it is not guaranteed stable across Rust
/// versions, and serialized filters depend on deterministic hashes.
fn hash_token(token: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for &b in token.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Shared tokenization pipeline: split on non-alphanumeric/non-`_`/non-`-`
/// boundaries, lowercase each piece, and discard tokens shorter than 2 characters.
///
/// This is the common first step for `tokenize`, `tokenize_ngrams`, and `gram_set`.
fn split_and_lowercase(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|tok| tok.to_lowercase())
        .filter(|tok| tok.len() >= 2)
}

/// Generate all contiguous 3-character substrings of a single lowercase token.
///
/// "exit" → \["exi", "xit"\]
/// "ab"   → \[\] (too short)
/// "abc"  → \["abc"\]
///
/// The returned substrings borrow from `token`. Tokens shorter than 3 characters
/// produce no grams; they are already covered by whole-token hashes.
fn ngrams_from_token(token: &str) -> impl Iterator<Item = &str> {
    let chars: Vec<(usize, char)> = token.char_indices().collect();
    let len = chars.len();
    (0..len.saturating_sub(2)).map(move |i| {
        let start = chars[i].0;
        let end = if i + 3 < len {
            chars[i + 3].0
        } else {
            token.len()
        };
        &token[start..end]
    })
}

/// Generate 3-gram hashes from text.
///
/// Splits text into tokens using the same rules as `tokenize` (split on
/// non-alphanumeric/underscore/hyphen, lowercase, filter >= 2 chars), then
/// for each token of length >= 3, generates all contiguous 3-character
/// substrings and hashes each one.
///
/// Tokens shorter than 3 characters produce no grams (they are already
/// covered by whole-token hashes in the combined filter).
///
/// Duplicate grams across tokens are deduplicated. For example, "exit" and
/// "exits" both contain the gram "exi" — it appears once in the output.
pub(crate) fn tokenize_ngrams(text: &str) -> Vec<u64> {
    let mut hashes: Vec<u64> = split_and_lowercase(text)
        .flat_map(|tok| {
            // ngrams_from_token borrows from tok, so collect into owned strings first.
            ngrams_from_token(&tok)
                .map(hash_token)
                .collect::<Vec<u64>>()
        })
        .collect();

    hashes.sort_unstable();
    hashes.dedup();
    hashes
}

/// Extract the set of 3-gram strings from text.
///
/// Same tokenization rules as `tokenize_ngrams`, but returns the actual
/// gram strings instead of hashes. Used by `tversky_score` for set
/// comparison.
///
/// Returns a `HashSet` for O(1) intersection operations.
#[allow(dead_code)]
pub(crate) fn gram_set(text: &str) -> HashSet<String> {
    let mut grams = HashSet::new();
    for tok in split_and_lowercase(text) {
        for gram in ngrams_from_token(&tok) {
            grams.insert(gram.to_owned());
        }
    }
    grams
}

/// Compute the Tversky similarity between a query and a document token.
///
/// Generates 3-gram sets from both strings and computes:
///
/// ```text
/// |intersection| / (|intersection| + a * |query_only| + b * |doc_only|)
/// ```
///
/// Uses a=1.0, b=0.0 (prototype model): the score measures what fraction
/// of the query's grams appear in the document token. The document can be
/// any length without penalty. This rewards partial recall queries (e.g.,
/// "hered" scores 1.0 against "heredoc") and penalizes only the query's
/// unmatched grams (typos reduce the score proportionally).
///
/// Returns 0.0 when either input produces no 3-grams.
#[allow(dead_code)]
pub(crate) fn tversky_score(query: &str, document: &str) -> f64 {
    let query_grams = gram_set(query);
    let doc_grams = gram_set(document);

    if query_grams.is_empty() {
        return 0.0;
    }

    let intersection = query_grams.intersection(&doc_grams).count();
    let query_only = query_grams.len() - intersection;

    // a=1.0, b=0.0: prototype model — only unmatched query grams penalize.
    // Denominator simplifies to |query_grams|: the fraction of query grams
    // found in the document.
    let denominator = intersection + query_only;
    if denominator == 0 {
        return 0.0;
    }

    intersection as f64 / denominator as f64
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::{assert_eq, assert_ne};
    use rstest::rstest;

    #[test]
    fn hello_world_produces_two_hashes() {
        let hashes = tokenize("Hello World");
        assert_eq!(hashes.len(), 2);
    }

    #[test]
    fn tokenization_is_case_insensitive() {
        assert_eq!(tokenize("Hello"), tokenize("hello"));
        assert_eq!(tokenize("WORLD"), tokenize("world"));
    }

    #[test]
    fn punctuation_splits_into_separate_tokens() {
        // "it's a test!" splits on ' and ! and spaces:
        // raw pieces -> ["it", "s", "a", "test", ""], filtered to >= 2 chars
        // -> ["it", "test"] (2 hashes)
        let hashes = tokenize("it's a test!");
        assert_eq!(hashes.len(), 2);
        let it = tokenize("it");
        let test = tokenize("test");
        assert_eq!(it.len(), 1);
        assert_eq!(test.len(), 1);
        assert!(hashes.contains(&it[0]));
        assert!(hashes.contains(&test[0]));
    }

    #[test]
    fn punctuation_boundary_splits_adjacent_words() {
        // "hello,world" has no space but the comma is a delimiter
        let hashes = tokenize("hello,world");
        assert_eq!(hashes.len(), 2);
        let hello = tokenize("hello");
        let world = tokenize("world");
        assert!(hashes.contains(&hello[0]));
        assert!(hashes.contains(&world[0]));
    }

    #[test]
    fn empty_text_returns_empty_vec() {
        assert_eq!(tokenize(""), Vec::<u64>::new());
    }

    #[test]
    fn whitespace_only_returns_empty_vec() {
        assert_eq!(tokenize("   "), Vec::<u64>::new());
        assert_eq!(tokenize("\t\n"), Vec::<u64>::new());
    }

    #[test]
    fn three_word_text_produces_three_hashes() {
        let hashes = tokenize("heredoc template placeholder");
        assert_eq!(hashes.len(), 3);
    }

    #[test]
    fn duplicate_tokens_deduplicated() {
        let once = tokenize("hello");
        let repeated = tokenize("hello hello hello");
        assert_eq!(once, repeated);
    }

    #[rstest]
    #[case("ab", 1)]
    #[case("a", 0)] // 1 char -> excluded
    #[case("", 0)]
    fn min_token_length_two_chars(#[case] input: &str, #[case] expected: usize) {
        assert_eq!(tokenize(input).len(), expected);
    }

    #[test]
    fn hash_token_is_deterministic() {
        assert_eq!(hash_token("hello"), hash_token("hello"));
        assert_eq!(hash_token("world"), hash_token("world"));
    }

    #[test]
    fn hash_token_distinct_tokens_produce_distinct_hashes() {
        // These are common words — collisions would be a red flag for FNV-1a
        let words = ["hello", "world", "test", "deploy", "rollback", "template"];
        let hashes: Vec<u64> = words.iter().map(|w| hash_token(w)).collect();
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            hashes.len(),
            "hash collision detected among common tokens"
        );
    }

    #[test]
    fn hyphen_and_underscore_kept_within_tokens() {
        // "rollback-plan" should tokenize as one token, not split on hyphen
        // "my_var" should be one token, not split on underscore
        let hyphen = tokenize("rollback-plan");
        let underscore = tokenize("my_var");
        assert_eq!(hyphen.len(), 1);
        assert_eq!(underscore.len(), 1);
    }

    // ── ngrams_from_token ─────────────────────────────────────────────────────

    #[test]
    fn ngrams_from_token_yields_sliding_window() {
        let token = "exit";
        let grams: Vec<&str> = ngrams_from_token(token).collect();
        assert_eq!(grams, vec!["exi", "xit"]);
    }

    #[test]
    fn ngrams_from_token_exactly_three_chars_yields_one_gram() {
        let token = "abc";
        let grams: Vec<&str> = ngrams_from_token(token).collect();
        assert_eq!(grams, vec!["abc"]);
    }

    #[test]
    fn ngrams_from_token_two_chars_yields_nothing() {
        let grams: Vec<&str> = ngrams_from_token("ab").collect();
        assert!(grams.is_empty());
    }

    #[test]
    fn ngrams_from_token_one_char_yields_nothing() {
        let grams: Vec<&str> = ngrams_from_token("a").collect();
        assert!(grams.is_empty());
    }

    // ── tokenize_ngrams ───────────────────────────────────────────────────────

    #[test]
    fn tokenize_ngrams_four_char_token_produces_two_hashes() {
        let hashes = tokenize_ngrams("exit");
        assert_eq!(hashes.len(), 2, "exit -> {{exi, xit}}");
    }

    #[test]
    fn tokenize_ngrams_five_char_token_produces_three_hashes() {
        let hashes = tokenize_ngrams("hello");
        assert_eq!(hashes.len(), 3, "hello -> {{hel, ell, llo}}");
    }

    #[test]
    fn tokenize_ngrams_two_char_token_produces_no_hashes() {
        assert!(tokenize_ngrams("ab").is_empty());
    }

    #[test]
    fn tokenize_ngrams_three_char_token_produces_one_hash() {
        let hashes = tokenize_ngrams("abc");
        assert_eq!(hashes.len(), 1);
    }

    #[test]
    fn tokenize_ngrams_empty_returns_empty() {
        assert!(tokenize_ngrams("").is_empty());
    }

    #[test]
    fn tokenize_ngrams_whitespace_only_returns_empty() {
        assert!(tokenize_ngrams("   ").is_empty());
    }

    #[test]
    fn tokenize_ngrams_is_case_insensitive() {
        assert_eq!(tokenize_ngrams("Hello"), tokenize_ngrams("hello"));
        assert_eq!(tokenize_ngrams("EXIT"), tokenize_ngrams("exit"));
    }

    #[test]
    fn tokenize_ngrams_multi_word_produces_grams_from_both_tokens() {
        // "hello world" should produce grams from "hello" (3) and "world" (3)
        // but "world" grams: wor, orl, rld -- no overlap with "hello" grams
        let both = tokenize_ngrams("hello world");
        let hello = tokenize_ngrams("hello");
        let world = tokenize_ngrams("world");
        // Union of hello and world grams (no overlap between them)
        assert_eq!(both.len(), hello.len() + world.len());
    }

    #[test]
    fn tokenize_ngrams_deduplicates_across_tokens() {
        // "hello hello" should produce the same hashes as "hello"
        let once = tokenize_ngrams("hello");
        let repeated = tokenize_ngrams("hello hello");
        assert_eq!(once, repeated);
    }

    #[test]
    fn tokenize_ngrams_short_tokens_after_split_produce_no_grams() {
        // "it's a test!" splits to: "it", "s", "a", "test"
        // Only "test" and "it" survive the >= 2 char filter.
        // "it" has 2 chars -> no grams. "test" has 4 chars -> 2 grams.
        let hashes = tokenize_ngrams("it's a test!");
        assert_eq!(hashes.len(), 2, "only 'test' produces grams: tes, est");
    }

    // ── gram_set ──────────────────────────────────────────────────────────────

    #[rstest]
    #[case::exit("exit", vec!["exi", "xit"])]
    #[case::heredoc("heredoc", vec!["her", "ere", "red", "edo", "doc"])]
    fn gram_set_produces_expected_grams(#[case] input: &str, #[case] expected: Vec<&str>) {
        let result = gram_set(input);
        let expected_set: HashSet<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(result, expected_set);
    }

    #[test]
    fn gram_set_two_char_input_is_empty() {
        assert!(gram_set("ab").is_empty());
    }

    #[test]
    fn gram_set_empty_input_is_empty() {
        assert!(gram_set("").is_empty());
    }

    #[test]
    fn gram_set_is_case_insensitive() {
        assert_eq!(gram_set("Hello"), gram_set("hello"));
    }

    // ── tversky_score ─────────────────────────────────────────────────────────

    #[test]
    fn tversky_score_partial_recall_query_fully_contained_in_doc() {
        // "hered" grams: {her, ere, red}; "heredoc" grams: {her, ere, red, edo, doc}
        // intersection=3, query_only=0 -> score = 3/3 = 1.0
        let score = tversky_score("hered", "heredoc");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn tversky_score_identical_strings() {
        assert_eq!(tversky_score("heredoc", "heredoc"), 1.0);
        assert_eq!(tversky_score("exit", "exit"), 1.0);
    }

    #[test]
    fn tversky_score_typo_returns_partial_score() {
        // "templete" grams: {tem, emp, mpl, ple, let, ete}
        // "template" grams: {tem, emp, mpl, pla, lat, ate}
        // intersection = {tem, emp, mpl} = 3, query_only = 3 -> score = 3/6 = 0.5
        let score = tversky_score("templete", "template");
        assert_eq!(score, 0.5);
    }

    #[test]
    fn tversky_score_single_transposition_four_char_token_returns_zero() {
        // "ecit" grams: {eci, cit}; "exit" grams: {exi, xit}
        // intersection = 0 -> score = 0.0
        let score = tversky_score("ecit", "exit");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn tversky_score_disjoint_grams_returns_zero() {
        assert_eq!(tversky_score("abc", "xyz"), 0.0);
    }

    #[test]
    fn tversky_score_query_too_short_for_grams_returns_zero() {
        // "ab" produces no grams -> 0.0
        assert_eq!(tversky_score("ab", "heredoc"), 0.0);
    }

    #[test]
    fn tversky_score_doc_grams_do_not_penalize() {
        // With b=0.0, extra grams in the document have no penalty.
        // "hered" scores 1.0 against "heredoc" despite "heredoc" having more grams.
        let short_score = tversky_score("hered", "heredoc");
        let _ = tversky_score("heredoc", "heredoc"); // identical -> also 1.0
        assert_eq!(short_score, 1.0);
    }

    #[test]
    fn tversky_score_is_asymmetric() {
        // With a=1.0, b=0.0 the score is not symmetric.
        // tversky("hered", "heredoc") = 1.0 but tversky("heredoc", "hered") < 1.0
        let forward = tversky_score("hered", "heredoc");
        let reverse = tversky_score("heredoc", "hered");
        assert_eq!(forward, 1.0);
        assert_ne!(reverse, 1.0, "prototype model is asymmetric");
    }
}
