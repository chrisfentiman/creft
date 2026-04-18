//! Indexed search primitives for creft skill documentation.
//!
//! Currently provides the [`xor::Xor8Filter`] probabilistic set-membership
//! filter. Tokenization, index serialization, and disk-based index lifecycle
//! are planned additions.

pub(crate) mod xor;
