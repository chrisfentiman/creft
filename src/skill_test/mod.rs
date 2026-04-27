//! Internals for `creft skills test`.
//!
//! Provides fixture discovery and parsing (Stage 2), sandbox lifecycle (Stage 3),
//! scenario orchestration (Stage 4), and coverage aggregation (future stages).
//! The public surface here is exposed to `cmd::skills`.

pub(crate) mod fixture;
