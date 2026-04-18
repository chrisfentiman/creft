//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, and a searchable
//! index format for fast approximate set membership queries across skill docs.

// Search primitives are not yet called from the binary entry points. The public API is exercised by module tests.
#[allow(dead_code)]
pub(crate) mod index;
#[allow(dead_code)]
pub(crate) mod tokenize;
#[allow(dead_code)]
pub(crate) mod xor;
