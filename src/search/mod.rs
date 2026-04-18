//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, a searchable index
//! format, and lifecycle management for per-namespace index files on disk.

pub(crate) mod index;
pub(crate) mod store;
#[allow(dead_code)]
pub(crate) mod tokenize;
#[allow(dead_code)]
pub(crate) mod xor;
