// The public API of this module is consumed by search/index.rs and later stages.
// The items are unused from the binary's perspective until the index is wired in.
#![allow(dead_code)]

/// Tokenize text into a set of 64-bit hashes.
///
/// Splits on whitespace, then for each word strips non-alphanumeric characters
/// (retaining `_` and `-` within tokens), lowercases the result, filters tokens
/// shorter than 2 chars, and hashes each unique token to a `u64` using FNV-1a.
///
/// Examples:
/// - `"Hello World"` → 2 hashes
/// - `"it's a test!"` → hashes for `"its"` and `"test"` (`"a"` is 1 char, stripped)
/// - `"rollback-plan"` → 1 hash (hyphen kept within token)
///
/// The hash function is FNV-1a (64-bit), separate from the SplitMix64 used
/// by the XOR filter internally. FNV-1a is fast for short strings and produces
/// good distribution for filter construction.
pub(crate) fn tokenize(text: &str) -> Vec<u64> {
    let mut hashes: Vec<u64> = text
        .split_whitespace()
        .map(|word| {
            // Strip non-alphanumeric chars that are not _ or - from each word,
            // then lowercase the result.
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|tok| tok.len() >= 2)
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
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
    fn punctuation_stripped_and_short_tokens_excluded() {
        // "it's a test!" -> "its", "a" (stripped, len 1), "test"
        // "a" is 1 char -> excluded; "its" and "test" remain
        let hashes = tokenize("it's a test!");
        // "its" and "test" — not "a"
        assert_eq!(hashes.len(), 2);
        // Those two hashes should match hashing "its" and "test" individually
        let its = tokenize("its");
        let test = tokenize("test");
        assert_eq!(its.len(), 1);
        assert_eq!(test.len(), 1);
        assert!(hashes.contains(&its[0]));
        assert!(hashes.contains(&test[0]));
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
    #[case("a", 0)]  // 1 char -> excluded
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
}
