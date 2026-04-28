//! Internals for `creft skills test`.
//!
//! Provides fixture discovery and parsing, sandbox lifecycle, scenario
//! orchestration, and coverage aggregation. The public surface here is
//! exposed to `cmd::skills`.

pub(crate) mod assertion;
pub(crate) mod coverage;
pub(crate) mod expand;
pub(crate) mod fixture;
pub(crate) mod match_pattern;
pub(crate) mod placeholder;
pub(crate) mod sandbox;
pub(crate) mod scenario;
