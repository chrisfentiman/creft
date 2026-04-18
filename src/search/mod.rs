//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, and a searchable
//! index format for fast approximate set membership queries across skill docs.

pub(crate) mod index;
pub(crate) mod tokenize;
pub(crate) mod xor;
