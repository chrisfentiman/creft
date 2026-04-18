//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, a searchable index
//! format, and lifecycle management for per-namespace index files on disk.

pub(crate) mod index;
pub(crate) mod store;
pub(crate) mod tokenize;
pub(crate) mod xor;
