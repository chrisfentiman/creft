// The public API of this module is consumed by search/index.rs (Stage 2).
// The items are unused from the binary's perspective until that stage is wired in.
#![allow(dead_code)]

/// An immutable Xor8 filter for approximate set membership testing.
///
/// Constructed from a set of `u64` keys. Membership queries never produce
/// false negatives (a key in the construction set always returns `true`).
/// False positives occur with probability ~1/256 (0.39%).
///
/// Memory: ~9 bits per key in the construction set (1.23 * n bytes for the
/// fingerprint array, plus 8 bytes for the seed and 4 for block_length).
#[derive(Debug)]
pub(crate) struct Xor8Filter {
    seed: u64,
    block_length: u32,
    fingerprints: Vec<u8>,
}

impl Xor8Filter {
    /// Build a filter from a set of 64-bit keys.
    ///
    /// Duplicate keys are tolerated — they are deduplicated internally.
    /// An empty key set produces a filter that rejects all queries.
    ///
    /// Construction may retry with a new seed if the initial mapping produces
    /// a cycle. Panics after 1024 retries, which is astronomically unlikely
    /// for any valid input.
    pub fn build(keys: &[u64]) -> Self {
        let mut keys: Vec<u64> = keys.to_vec();
        keys.sort_unstable();
        keys.dedup();

        let n = keys.len();
        if n == 0 {
            return Self {
                seed: 0,
                block_length: 0,
                fingerprints: Vec::new(),
            };
        }

        // Capacity formula from the reference Go implementation:
        // 32 + ceil(1.23 * n), rounded DOWN to a multiple of 3.
        let raw = 32 + ((n as f64 * 1.23).ceil() as usize);
        let capacity = raw - (raw % 3); // round down to multiple of 3
        let block_length = (capacity / 3) as u32;

        // Seed generation: use a counter fed through SplitMix64, matching the
        // reference implementation's `rngcounter` approach.
        let mut rng: u64 = 1;

        for _ in 0..1024 {
            let seed = splitmix64_advance(&mut rng);
            if let Some(fingerprints) = try_build(&keys, seed, block_length) {
                return Self {
                    seed,
                    block_length,
                    fingerprints,
                };
            }
        }

        panic!("Xor8Filter::build failed after 1024 attempts — this should never happen");
    }

    /// Test whether a key is probably in the set.
    ///
    /// Returns `true` for all construction keys (no false negatives) and
    /// returns `true` for non-members with probability ~0.39%.
    pub fn contains(&self, key: u64) -> bool {
        if self.block_length == 0 {
            return false;
        }
        let hash = mixsplit(key, self.seed);
        let (h0, h1, h2) = hash_to_positions(hash, self.block_length);
        let fp = fingerprint8(hash);
        fp == self.fingerprints[h0 as usize]
            ^ self.fingerprints[h1 as usize]
            ^ self.fingerprints[h2 as usize]
    }

    /// Serialize the filter to a byte vector.
    ///
    /// Format: `[seed: 8 bytes LE][block_length: 4 bytes LE][fingerprints...]`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.fingerprints.len());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&self.block_length.to_le_bytes());
        out.extend_from_slice(&self.fingerprints);
        out
    }

    /// Deserialize a filter from a byte slice.
    ///
    /// Returns `None` if the slice is shorter than the 12-byte header or if
    /// the fingerprint array length does not match the header.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }
        let seed = u64::from_le_bytes(data[0..8].try_into().ok()?);
        let block_length = u32::from_le_bytes(data[8..12].try_into().ok()?);

        if block_length == 0 {
            // Empty-set filter: 12-byte header only.
            if data.len() == 12 {
                return Some(Self {
                    seed,
                    block_length: 0,
                    fingerprints: Vec::new(),
                });
            }
            return None;
        }

        let expected_fp_len = (block_length as usize).checked_mul(3)?;
        if data.len() != 12 + expected_fp_len {
            return None;
        }

        Some(Self {
            seed,
            block_length,
            fingerprints: data[12..].to_vec(),
        })
    }

    /// Number of fingerprint entries in the filter.
    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }
}

// ── construction ──────────────────────────────────────────────────────────────

/// Per-slot accumulator used during construction.
#[derive(Clone)]
struct Slot {
    /// XOR of the key hashes (`mixsplit(key, seed)`) of all keys currently
    /// mapped to this slot. When `count == 1`, this is the hash of the one
    /// remaining key — sufficient to re-derive all three positions.
    xormask: u64,
    /// Number of keys currently assigned to this slot.
    count: u32,
}

/// Entry in the peeling stack.
///
/// Stores the fingerprint-array index (absolute, not block-relative) and
/// the key hash of the key that was peeled at that step. The hash is enough
/// to recompute all three positions during fingerprint assignment.
struct StackEntry {
    /// Absolute index into the fingerprint array where this key was peeled.
    index: u32,
    /// `mixsplit(key, seed)` for the peeled key.
    hash: u64,
}

/// Attempt one construction pass with a given seed.
///
/// Returns `Some(fingerprints)` on success, or `None` if the hypergraph
/// contains a cycle (caller should retry with a different seed).
fn try_build(keys: &[u64], seed: u64, block_length: u32) -> Option<Vec<u8>> {
    let n = keys.len();
    let array_len = (block_length * 3) as usize;

    // Three independent arrays of slots, one per hash function.
    let mut sets = vec![
        Slot {
            xormask: 0,
            count: 0,
        };
        array_len
    ];

    // Populate: map every key into its three slots, accumulating XOR masks.
    for &key in keys {
        let hash = mixsplit(key, seed);
        let (h0, h1, h2) = hash_to_positions(hash, block_length);
        sets[h0 as usize].xormask ^= hash;
        sets[h0 as usize].count += 1;
        sets[h1 as usize].xormask ^= hash;
        sets[h1 as usize].count += 1;
        sets[h2 as usize].xormask ^= hash;
        sets[h2 as usize].count += 1;
    }

    // Peeling worklists: one per block so we can process block-local singletons.
    let bl = block_length as usize;
    let mut q0: Vec<u32> = (0..bl as u32)
        .filter(|&i| sets[i as usize].count == 1)
        .collect();
    let mut q1: Vec<u32> = (bl as u32..(2 * bl) as u32)
        .filter(|&i| sets[i as usize].count == 1)
        .collect();
    let mut q2: Vec<u32> = ((2 * bl) as u32..(3 * bl) as u32)
        .filter(|&i| sets[i as usize].count == 1)
        .collect();

    let mut stack: Vec<StackEntry> = Vec::with_capacity(n);

    loop {
        // Drain q0 singletons.
        while let Some(idx) = q0.pop() {
            if sets[idx as usize].count == 0 {
                continue;
            }
            let hash = sets[idx as usize].xormask;
            let (_, h1, h2) = hash_to_positions(hash, block_length);
            stack.push(StackEntry { index: idx, hash });
            sets[idx as usize].count = 0;

            sets[h1 as usize].xormask ^= hash;
            sets[h1 as usize].count -= 1;
            if sets[h1 as usize].count == 1 {
                q1.push(h1);
            }
            sets[h2 as usize].xormask ^= hash;
            sets[h2 as usize].count -= 1;
            if sets[h2 as usize].count == 1 {
                q2.push(h2);
            }
        }

        // Drain q1 singletons.
        while let Some(idx) = q1.pop() {
            if sets[idx as usize].count == 0 {
                continue;
            }
            let hash = sets[idx as usize].xormask;
            let (h0, _, h2) = hash_to_positions(hash, block_length);
            stack.push(StackEntry { index: idx, hash });
            sets[idx as usize].count = 0;

            sets[h0 as usize].xormask ^= hash;
            sets[h0 as usize].count -= 1;
            if sets[h0 as usize].count == 1 {
                q0.push(h0);
            }
            sets[h2 as usize].xormask ^= hash;
            sets[h2 as usize].count -= 1;
            if sets[h2 as usize].count == 1 {
                q2.push(h2);
            }
        }

        // Drain q2 singletons.
        while let Some(idx) = q2.pop() {
            if sets[idx as usize].count == 0 {
                continue;
            }
            let hash = sets[idx as usize].xormask;
            let (h0, h1, _) = hash_to_positions(hash, block_length);
            stack.push(StackEntry { index: idx, hash });
            sets[idx as usize].count = 0;

            sets[h0 as usize].xormask ^= hash;
            sets[h0 as usize].count -= 1;
            if sets[h0 as usize].count == 1 {
                q0.push(h0);
            }
            sets[h1 as usize].xormask ^= hash;
            sets[h1 as usize].count -= 1;
            if sets[h1 as usize].count == 1 {
                q1.push(h1);
            }
        }

        // If all queues are empty, peeling has completed (success) or stalled (cycle).
        if q0.is_empty() && q1.is_empty() && q2.is_empty() {
            break;
        }
    }

    if stack.len() != n {
        return None;
    }

    // Assign fingerprints in reverse peeling order.
    //
    // Invariant: for every key k with hash h and positions (h0, h1, h2),
    //   fingerprints[h0] ^ fingerprints[h1] ^ fingerprints[h2] == fingerprint8(h).
    let mut fingerprints = vec![0u8; array_len];
    for entry in stack.into_iter().rev() {
        let (h0, h1, h2) = hash_to_positions(entry.hash, block_length);
        let fp = fingerprint8(entry.hash);
        fingerprints[entry.index as usize] =
            fp ^ fingerprints[h0 as usize] ^ fingerprints[h1 as usize] ^ fingerprints[h2 as usize];
    }

    Some(fingerprints)
}

// ── hash primitives ───────────────────────────────────────────────────────────

/// SplitMix64: advance the RNG counter and return the next value.
///
/// Used to generate successive seeds during construction retries, matching
/// the reference implementation's `rngcounter` approach.
fn splitmix64_advance(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Mix a key with the filter seed to produce a 64-bit hash.
///
/// Uses SplitMix64 as in the reference implementation.
fn mixsplit(key: u64, seed: u64) -> u64 {
    let mut x = key.wrapping_add(seed);
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Derive the 8-bit fingerprint from a key hash.
fn fingerprint8(hash: u64) -> u8 {
    (hash ^ (hash >> 32)) as u8
}

/// Fastrange reduction: maps a `u32` uniformly into `[0, range)`.
fn reduce(hash: u32, range: u32) -> u32 {
    ((hash as u64 * range as u64) >> 32) as u32
}

/// Rotate a 64-bit value left by `n` bits.
fn rotl64(x: u64, n: u32) -> u64 {
    x.rotate_left(n)
}

/// Derive the three absolute fingerprint-array positions from a key hash.
///
/// Uses bit rotation (not right-shift) to extract three independent 32-bit
/// values from the 64-bit hash, following the reference implementation.
/// Each position falls in a different block, so h0, h1, h2 are always distinct.
fn hash_to_positions(hash: u64, block_length: u32) -> (u32, u32, u32) {
    let r0 = hash as u32;
    let r1 = rotl64(hash, 21) as u32;
    let r2 = rotl64(hash, 42) as u32;
    let h0 = reduce(r0, block_length);
    let h1 = reduce(r1, block_length) + block_length;
    let h2 = reduce(r2, block_length) + 2 * block_length;
    (h0, h1, h2)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::{assert_eq, assert_ne};

    // ── hash primitives ───────────────────────────────────────────────────────

    #[test]
    fn mixsplit_is_deterministic() {
        assert_eq!(mixsplit(42, 0), mixsplit(42, 0));
        assert_eq!(mixsplit(0, 99), mixsplit(0, 99));
    }

    #[test]
    fn mixsplit_distinct_inputs_distinct_outputs() {
        assert_ne!(mixsplit(0, 0), mixsplit(1, 0));
        assert_ne!(mixsplit(42, 0), mixsplit(42, 1));
    }

    #[test]
    fn reduce_output_always_in_range() {
        for range in [1u32, 7, 100, 1000, 65535] {
            for hash in [0u32, 1, 12345, u32::MAX] {
                let r = reduce(hash, range);
                assert!(
                    r < range,
                    "reduce({hash}, {range}) = {r} out of [0, {range})"
                );
            }
        }
    }

    #[test]
    fn hash_to_positions_each_in_correct_block() {
        let bl = 100u32;
        let (h0, h1, h2) = hash_to_positions(mixsplit(42, 0), bl);
        assert!(h0 < bl, "h0={h0} not in block 0 [0, {bl})");
        assert!(
            h1 >= bl && h1 < 2 * bl,
            "h1={h1} not in block 1 [{bl}, {})",
            2 * bl
        );
        assert!(
            h2 >= 2 * bl && h2 < 3 * bl,
            "h2={h2} not in block 2 [{}, {})",
            2 * bl,
            3 * bl
        );
    }

    #[test]
    fn fingerprint8_is_deterministic() {
        assert_eq!(fingerprint8(42), fingerprint8(42));
        assert_eq!(fingerprint8(0), fingerprint8(0));
    }

    // ── build + contains ──────────────────────────────────────────────────────

    #[test]
    fn all_construction_keys_are_present() {
        let keys = [1u64, 2, 3, 4, 5];
        let filter = Xor8Filter::build(&keys);
        for &k in &keys {
            assert!(
                filter.contains(k),
                "construction key {k} not found in filter"
            );
        }
    }

    #[test]
    fn empty_set_rejects_every_query() {
        let filter = Xor8Filter::build(&[]);
        for key in [0u64, 1, 42, u64::MAX] {
            assert!(!filter.contains(key), "empty filter should reject {key}");
        }
    }

    #[test]
    fn single_key_present_and_neighbor_likely_absent() {
        let filter = Xor8Filter::build(&[42]);
        assert!(filter.contains(42));
        assert!(!filter.contains(43));
    }

    #[test]
    fn duplicate_keys_treated_as_deduplicated_set() {
        let filter = Xor8Filter::build(&[1, 1, 1, 2, 2]);
        assert!(filter.contains(1));
        assert!(filter.contains(2));
    }

    #[test]
    fn false_positive_rate_below_one_percent() {
        // 100 construction keys, 10 000 non-member probes.
        let construction: Vec<u64> = (0..100).map(|i| i * 1_000_000).collect();
        let filter = Xor8Filter::build(&construction);

        let probes: Vec<u64> = (1..=10_000).map(|i| i * 1_000_000 + 1).collect();
        let fp_count = probes.iter().filter(|&&k| filter.contains(k)).count();
        let fp_rate = fp_count as f64 / probes.len() as f64;

        assert!(
            fp_rate < 0.01,
            "false positive rate {fp_rate:.4} exceeds 1% ({fp_count}/{} positives)",
            probes.len()
        );
    }

    // ── serialization round-trip ──────────────────────────────────────────────

    #[test]
    fn round_trip_all_construction_keys_survive() {
        let keys: Vec<u64> = (1..=20).collect();
        let filter = Xor8Filter::build(&keys);
        let restored = Xor8Filter::from_bytes(&filter.to_bytes())
            .expect("round-trip deserialization should succeed");

        for &k in &keys {
            assert!(restored.contains(k), "key {k} missing after round-trip");
        }
    }

    #[test]
    fn empty_filter_round_trips_correctly() {
        let filter = Xor8Filter::build(&[]);
        let bytes = filter.to_bytes();
        let restored = Xor8Filter::from_bytes(&bytes).expect("empty filter should deserialize");
        assert!(!restored.contains(0));
        assert!(!restored.contains(u64::MAX));
    }

    #[test]
    fn from_bytes_rejects_too_short_input() {
        assert!(Xor8Filter::from_bytes(&[]).is_none());
        assert!(Xor8Filter::from_bytes(&[0u8; 11]).is_none());
    }

    #[test]
    fn from_bytes_rejects_truncated_fingerprint_array() {
        let filter = Xor8Filter::build(&[1, 2, 3, 4, 5]);
        let mut bytes = filter.to_bytes();
        bytes.truncate(bytes.len() - 1);
        assert!(Xor8Filter::from_bytes(&bytes).is_none());
    }

    #[test]
    fn serialization_byte_length_matches_header_plus_fingerprints() {
        let keys: Vec<u64> = (0..100).collect();
        let filter = Xor8Filter::build(&keys);
        let bytes = filter.to_bytes();
        assert_eq!(bytes.len(), 12 + filter.len());
    }

    #[test]
    fn round_trip_preserves_false_positive_behavior() {
        let construction: Vec<u64> = (0..50).map(|i| i * 999_983).collect();
        let filter = Xor8Filter::build(&construction);
        let restored = Xor8Filter::from_bytes(&filter.to_bytes()).unwrap();

        let probes: Vec<u64> = (1..=10_000).map(|i| i * 999_983 + 1).collect();
        let fp_before: Vec<u64> = probes
            .iter()
            .copied()
            .filter(|&k| filter.contains(k))
            .collect();
        let fp_after: Vec<u64> = probes
            .iter()
            .copied()
            .filter(|&k| restored.contains(k))
            .collect();

        assert_eq!(
            fp_before, fp_after,
            "false positive sets differ after round-trip"
        );
    }

    // ── larger inputs ─────────────────────────────────────────────────────────

    #[test]
    fn large_key_set_all_construction_keys_present() {
        let keys: Vec<u64> = (0..1000).collect();
        let filter = Xor8Filter::build(&keys);
        for &k in &keys {
            assert!(filter.contains(k), "key {k} missing in large filter");
        }
    }
}
