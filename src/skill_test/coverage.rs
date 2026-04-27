//! Coverage assertion checking against the runtime trace records.
//!
//! Coverage expectations use minimum-count semantics: an assertion that block 0
//! called `store_put` at least once passes whenever the trace shows `store_put >= 1`.
//! Exact-count assertions are not supported in v1 — they are brittle and rarely
//! necessary in practice.

use crate::runner::TraceRecord;
use crate::skill_test::assertion::AssertionFailure;
use crate::skill_test::fixture::CoverageExpectation;

/// Check coverage expectations against the captured trace records.
///
/// When `expected.blocks` contains an index that is absent from `actual`,
/// that block is reported as a failure. When `expected.primitives` requires a
/// minimum count that the trace does not satisfy, that primitive is reported.
///
/// If `actual` is empty and any coverage is expected, returns a single
/// diagnostic failure explaining the likely cause rather than enumerating every
/// missing block.
pub(crate) fn check_coverage(
    expected: &CoverageExpectation,
    actual: &[TraceRecord],
) -> Vec<AssertionFailure> {
    // Nothing to check when there are no expectations.
    if expected.blocks.is_empty() && expected.primitives.is_empty() {
        return Vec::new();
    }

    // Empty trace with non-empty expectations: surface one clear diagnostic.
    if actual.is_empty() {
        return vec![AssertionFailure {
            kind: "coverage",
            expected: "trace records".to_owned(),
            actual: "no trace recorded — argv[0] may not be a creft binary, or the child exited before any block completed".to_owned(),
            locator: None,
        }];
    }

    let mut failures = Vec::new();

    // Check block presence.
    for &block_idx in &expected.blocks {
        if !actual.iter().any(|r| r.block == block_idx) {
            failures.push(AssertionFailure {
                kind: "coverage",
                expected: format!("block {} to have executed", block_idx),
                actual: format!(
                    "block {} not found in trace (executed blocks: {})",
                    block_idx,
                    format_block_list(actual),
                ),
                locator: Some(format!("block {}", block_idx)),
            });
        }
    }

    // Check primitive minimum counts.
    for (block_idx, prim_expectations) in &expected.primitives {
        let record = actual.iter().find(|r| r.block == *block_idx);

        for (prim_name, &min_count) in prim_expectations {
            let actual_count = record
                .and_then(|r| r.primitives.get(prim_name))
                .copied()
                .unwrap_or(0);

            if actual_count < min_count {
                failures.push(AssertionFailure {
                    kind: "coverage",
                    expected: format!(
                        "block {} to call `{}` at least {} time(s)",
                        block_idx, prim_name, min_count
                    ),
                    actual: format!(
                        "block {} called `{}` {} time(s)",
                        block_idx, prim_name, actual_count
                    ),
                    locator: Some(format!("block {}", block_idx)),
                });
            }
        }
    }

    failures
}

/// Format the set of block indices present in the trace for error messages.
fn format_block_list(records: &[TraceRecord]) -> String {
    let mut indices: Vec<usize> = records.iter().map(|r| r.block).collect();
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() {
        return "none".to_owned();
    }
    indices
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::*;

    fn record(block: usize, lang: &str, exit: i32, primitives: &[(&str, u32)]) -> TraceRecord {
        TraceRecord {
            block,
            lang: lang.to_owned(),
            exit,
            primitives: primitives
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
        }
    }

    fn expectation(
        blocks: &[usize],
        primitives: &[(usize, &[(&str, u32)])],
    ) -> CoverageExpectation {
        CoverageExpectation {
            blocks: blocks.to_vec(),
            primitives: primitives
                .iter()
                .map(|(idx, prims)| {
                    let map: BTreeMap<String, u32> =
                        prims.iter().map(|(k, v)| (k.to_string(), *v)).collect();
                    (*idx, map)
                })
                .collect(),
        }
    }

    // ── Empty expectations ────────────────────────────────────────────────────

    #[test]
    fn empty_expectations_always_passes() {
        let exp = CoverageExpectation::default();
        let trace = vec![record(0, "bash", 0, &[("print", 3)])];
        assert!(check_coverage(&exp, &trace).is_empty());
    }

    // ── Empty trace ───────────────────────────────────────────────────────────

    #[test]
    fn non_empty_expectations_against_empty_trace_returns_single_diagnostic() {
        let exp = expectation(&[0], &[]);
        let failures = check_coverage(&exp, &[]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "coverage");
        assert!(failures[0].actual.contains("no trace recorded"));
    }

    // ── Block presence ────────────────────────────────────────────────────────

    #[test]
    fn block_present_passes() {
        let exp = expectation(&[0, 1], &[]);
        let trace = vec![record(0, "bash", 0, &[]), record(1, "python", 0, &[])];
        assert!(check_coverage(&exp, &trace).is_empty());
    }

    #[test]
    fn missing_block_reported() {
        let exp = expectation(&[0, 1], &[]);
        let trace = vec![record(0, "bash", 0, &[])];
        let failures = check_coverage(&exp, &trace);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "coverage");
        assert!(failures[0].expected.contains("block 1"));
    }

    // ── Primitive counts ──────────────────────────────────────────────────────

    #[test]
    fn primitive_min_count_met() {
        let exp = expectation(&[], &[(1, &[("store_put", 1)])]);
        let trace = vec![record(1, "bash", 0, &[("store_put", 2)])];
        assert!(check_coverage(&exp, &trace).is_empty());
    }

    #[test]
    fn primitive_min_count_not_met() {
        let exp = expectation(&[], &[(1, &[("store_put", 1)])]);
        let trace = vec![record(1, "bash", 0, &[("store_put", 0)])];
        let failures = check_coverage(&exp, &trace);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].expected.contains("store_put"));
    }

    #[test]
    fn primitive_absent_from_trace_fails() {
        let exp = expectation(&[], &[(0, &[("print", 1)])]);
        let trace = vec![record(0, "bash", 0, &[])];
        let failures = check_coverage(&exp, &trace);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].actual.contains("0 time(s)"));
    }

    #[test]
    fn multiple_blocks_combined_blocks_and_primitives() {
        let exp = expectation(&[0, 1], &[(0, &[("print", 1)]), (1, &[("store_put", 1)])]);
        let trace = vec![
            record(0, "bash", 0, &[("print", 2)]),
            record(1, "python", 0, &[("store_put", 1)]),
        ];
        assert!(check_coverage(&exp, &trace).is_empty());
    }

    #[test]
    fn reports_all_failures_in_one_pass() {
        // Block 0 is present, block 2 is missing.
        // Block 0 print=0 but expected 1.
        let exp = expectation(&[0, 2], &[(0, &[("print", 1)])]);
        let trace = vec![record(0, "bash", 0, &[("print", 0)])];
        let failures = check_coverage(&exp, &trace);
        // One for missing block 2, one for the print count.
        assert_eq!(failures.len(), 2);
    }
}
