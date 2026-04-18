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
/// "abc"  → \["abc"\]
/// "ab"   → \[\] (too short for a trigram)
/// "a"    → \[\] (too short)
///
/// The returned substrings borrow from `token`. Tokens shorter than 3 characters
/// produce no grams; they are already covered by whole-token hashes.
fn ngrams_from_token(token: &str) -> impl Iterator<Item = &str> {
    const N: usize = 3;
    let chars: Vec<(usize, char)> = token.char_indices().collect();
    let len = chars.len();
    (0..len.saturating_sub(N - 1)).map(move |i| {
        let start = chars[i].0;
        let end = if i + N < len {
            chars[i + N].0
        } else {
            token.len()
        };
        &token[start..end]
    })
}

/// Generate trigram hashes from text.
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
/// "exits" both contain the gram "xit" — it appears once in the output.
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

/// Extract the set of trigram strings from text.
///
/// Same tokenization rules as `tokenize_ngrams`, but returns the actual
/// gram strings instead of hashes. Used by `tversky_score` for set
/// comparison.
///
/// Returns a `HashSet` for O(1) intersection operations.
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
/// Generates trigram sets from both strings and computes:
///
/// ```text
/// |intersection| / (|intersection| + a * |query_only| + b * |doc_only|)
/// ```
///
/// Uses a=1.0, b=0.0 (prototype model): the score measures what fraction
/// of the query's grams appear in the document token. The document can be
/// any length without penalty. This rewards partial recall queries (e.g.,
/// "here" scores 1.0 against "heredoc") and penalizes only the query's
/// unmatched grams (typos reduce the score proportionally).
///
/// Returns 0.0 when either input produces no trigrams.
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

/// Score a multi-word query against a document's full text.
///
/// For each query word (split on non-alphanumeric/underscore/hyphen, lowercase,
/// length >= 2), computes `tversky_score` against every word in the document
/// text that meets the same length threshold, and takes the best per-word score.
/// Returns the average of per-word best scores.
///
/// A document that contains good matches for all query words scores close to 1.0.
/// A document that matches only some words scores proportionally lower.
///
/// Returns 0.0 when the query is empty or all query words are shorter than 3
/// characters (no trigrams can be generated to compare).
pub(crate) fn score_query(query: &str, document_text: &str) -> f64 {
    score_query_with_matches(query, document_text).0
}

/// Score a multi-word query and return the best-matching document word per query word.
///
/// Same scoring logic as [`score_query`], but also returns the document word that
/// achieved the highest Tversky score for each query word. The returned words are
/// in document-normalized form (lowercased), one per query word that produced a
/// score > 0.0.
///
/// These matched words are real tokens from the document, so they appear as
/// substrings of document lines and can be used for snippet extraction on the
/// fuzzy path — unlike the original (possibly misspelled) query terms, which may
/// not be substrings of any line.
///
/// Returns `(0.0, vec![])` when the query is empty or all query words are shorter
/// than 2 characters.
pub(crate) fn score_query_with_matches(query: &str, document_text: &str) -> (f64, Vec<String>) {
    let query_words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect();

    if query_words.is_empty() {
        return (0.0, Vec::new());
    }

    let doc_words: Vec<String> = document_text
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect();

    if doc_words.is_empty() {
        return (0.0, Vec::new());
    }

    let mut total = 0.0_f64;
    let mut matched_words: Vec<String> = Vec::with_capacity(query_words.len());

    for qw in &query_words {
        let (best_score, best_word) = doc_words.iter().map(|dw| (tversky_score(qw, dw), dw)).fold(
            (0.0_f64, None),
            |(best_s, best_w), (s, dw)| {
                if s > best_s {
                    (s, Some(dw))
                } else {
                    (best_s, best_w)
                }
            },
        );
        total += best_score;
        if let Some(word) = best_word
            && best_score > 0.0
        {
            matched_words.push(word.clone());
        }
    }

    // Deduplicate matched words — different query words may map to the same
    // document word, and we only need each match target once for snippet extraction.
    matched_words.sort_unstable();
    matched_words.dedup();

    (total / query_words.len() as f64, matched_words)
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
    fn ngrams_from_token_four_chars_yields_two_grams() {
        let token = "abcd";
        let grams: Vec<&str> = ngrams_from_token(token).collect();
        assert_eq!(grams, vec!["abc", "bcd"]);
    }

    #[test]
    fn ngrams_from_token_two_chars_yields_nothing() {
        let grams: Vec<&str> = ngrams_from_token("ab").collect();
        assert!(grams.is_empty(), "trigrams require at least 3 characters");
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
        // Trigrams require at least 3 characters; "ab" produces nothing.
        let hashes = tokenize_ngrams("ab");
        assert_eq!(hashes.len(), 0, "ab produces no trigrams");
    }

    #[test]
    fn tokenize_ngrams_three_char_token_produces_one_hash() {
        let hashes = tokenize_ngrams("abc");
        assert_eq!(hashes.len(), 1, "abc -> {{abc}}");
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
        // "hello" grams: hel, ell, llo; "world" grams: wor, orl, rld — no overlap
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
    fn tokenize_ngrams_short_tokens_after_split_produce_grams() {
        // "it's a test!" splits to: "it", "s", "a", "test"
        // "test" (4 chars) survives and produces 2 trigrams: {tes, est}.
        // "it" (2 chars), "s", and "a" are all too short for trigrams.
        let hashes = tokenize_ngrams("it's a test!");
        assert_eq!(hashes.len(), 2, "'test' produces {{tes, est}}; 'it' is too short");
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
    fn gram_set_two_char_input_yields_no_grams() {
        // Two characters cannot form a trigram.
        let result = gram_set("ab");
        assert!(result.is_empty(), "two-char input produces no trigrams");
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
        // "hered" trigrams: {her, ere, red}; "heredoc" trigrams: {her, ere, red, edo, doc}
        // intersection=3, query_only=0 -> score = 3/3 = 1.0
        let score = tversky_score("hered", "heredoc");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn tversky_score_identical_strings() {
        assert_eq!(tversky_score("heredoc", "heredoc"), 1.0);
        // "exit" only has 2 trigrams (exi, xit), still identical -> 1.0
        assert_eq!(tversky_score("exit", "exit"), 1.0);
    }

    #[test]
    fn tversky_score_typo_returns_partial_score() {
        // "templete" trigrams: {tem, emp, mpl, ple, let, ete}
        // "template" trigrams: {tem, emp, mpl, pla, lat, ate}
        // intersection = {tem, emp, mpl} = 3, query_only = {ple, let, ete} = 3 -> score = 3/6 = 0.5
        let score = tversky_score("templete", "template");
        assert_eq!(score, 3.0 / 6.0);
    }

    #[test]
    fn tversky_score_single_transposition_four_char_token_no_shared_trigrams() {
        // "ecit" trigrams: {eci, cit}; "exit" trigrams: {exi, xit}
        // No shared trigrams: intersection = 0, score = 0.0
        // Trigrams do not catch single-transposition typos in 4-char tokens —
        // longer queries (5+ chars) retain enough shared trigrams to score well.
        let score = tversky_score("ecit", "exit");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn tversky_score_typo_five_char_token_shares_trigrams() {
        // "exitt" trigrams: {exi, xit, itt}; "exit" trigrams: {exi, xit}
        // intersection = {exi, xit} = 2, query_only = {itt} = 1 -> score = 2/3
        let score = tversky_score("exitt", "exit");
        assert_eq!(score, 2.0 / 3.0);
    }

    #[test]
    fn tversky_score_disjoint_grams_returns_zero() {
        assert_eq!(tversky_score("abc", "xyz"), 0.0);
    }

    #[test]
    fn tversky_score_no_shared_trigrams_returns_zero() {
        // "ab" has no trigrams (too short); "heredoc" has {her, ere, red, edo, doc} — no overlap
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
        // "hered" trigrams are a subset of "heredoc" trigrams, so forward = 1.0.
        // "heredoc" has trigrams {edo, doc} not in "hered", so reverse < 1.0.
        let forward = tversky_score("hered", "heredoc");
        let reverse = tversky_score("heredoc", "hered");
        assert_eq!(forward, 1.0);
        assert_ne!(reverse, 1.0, "prototype model is asymmetric");
    }

    // ── score_query ───────────────────────────────────────────────────────────

    #[test]
    fn score_query_single_word_best_match_in_document() {
        // "hered" trigrams fully contained in "heredoc" trigrams -> score 1.0
        let score = score_query("hered", "this explains the heredoc syntax");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn score_query_multi_word_averages_per_word_best_scores() {
        // "hered" trigrams fully contained in "heredoc" trigrams -> 1.0
        // "templete" vs "template": intersection=3, query_only=3 -> 3/6 = 0.5
        // average = (1.0 + 0.5) / 2
        let score = score_query("hered templete", "the heredoc template guide");
        assert_eq!(score, (1.0 + 3.0 / 6.0) / 2.0);
    }

    #[test]
    fn score_query_no_match_returns_zero() {
        let score = score_query("zzz", "nothing matches here");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn score_query_single_char_query_word_returns_zero() {
        // "a" is < 2 chars and is filtered out, leaving no query words
        let score = score_query("a", "anything");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn score_query_empty_query_returns_zero() {
        let score = score_query("", "some document text here");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn score_query_empty_document_returns_zero() {
        let score = score_query("heredoc", "");
        assert_eq!(score, 0.0);
    }

    // ── score_query_with_matches ──────────────────────────────────────────────

    #[test]
    fn score_query_with_matches_returns_same_score_as_score_query() {
        // score_query delegates to score_query_with_matches; scores must agree.
        let doc = "the heredoc template guide";
        let query = "hered templete";
        let (score, _) = score_query_with_matches(query, doc);
        assert_eq!(score, score_query(query, doc));
    }

    #[test]
    fn score_query_with_matches_returns_best_matching_doc_word() {
        // "templete" trigrams: {tem, emp, mpl, ple, let, ete}
        // "template" trigrams: {tem, emp, mpl, pla, lat, ate}
        // intersection=3, query_only=3 -> score = 0.5 > 0.0
        // The matched word must be "template", not the query typo "templete".
        let (score, matched) = score_query_with_matches("templete", "use the template command");
        assert!(score > 0.0, "templete must score > 0.0 against 'template'");
        assert!(
            matched.contains(&"template".to_owned()),
            "matched words must contain 'template', got {:?}",
            matched
        );
        assert!(
            !matched.contains(&"templete".to_owned()),
            "matched words must not contain the query typo 'templete'"
        );
    }

    #[test]
    fn score_query_with_matches_multi_word_returns_one_match_per_query_word() {
        // "hered" → "heredoc"; "templete" → "template"
        let (_, matched) = score_query_with_matches("hered templete", "the heredoc template guide");
        assert!(
            matched.contains(&"heredoc".to_owned()),
            "must match 'heredoc' for query word 'hered'"
        );
        assert!(
            matched.contains(&"template".to_owned()),
            "must match 'template' for query word 'templete'"
        );
    }

    #[test]
    fn score_query_with_matches_deduplicates_same_doc_word() {
        // Two query words both best-match the same document word — only one copy.
        let (_, matched) = score_query_with_matches("roollback roollback", "rollback procedure");
        let rollback_count = matched.iter().filter(|w| w.as_str() == "rollback").count();
        assert_eq!(
            rollback_count, 1,
            "duplicate matched words must be deduplicated"
        );
    }

    #[test]
    fn score_query_with_matches_empty_query_returns_empty_matches() {
        let (score, matched) = score_query_with_matches("", "anything here");
        assert_eq!(score, 0.0);
        assert!(matched.is_empty());
    }

    #[test]
    fn score_query_with_matches_empty_document_returns_empty_matches() {
        let (score, matched) = score_query_with_matches("heredoc", "");
        assert_eq!(score, 0.0);
        assert!(matched.is_empty());
    }

    #[test]
    fn score_query_with_matches_zero_score_words_excluded_from_matches() {
        // "zzz" has no trigram overlap with any word in the document.
        // The matched words list must be empty even though doc words exist.
        let (score, matched) = score_query_with_matches("zzz", "nothing matches here");
        assert_eq!(score, 0.0);
        assert!(
            matched.is_empty(),
            "words with 0.0 score must not appear in matched words"
        );
    }
}
