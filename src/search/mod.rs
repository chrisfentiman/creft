//! Indexed search primitives for creft skill documentation.
//!
//! Provides text tokenization, XOR filter construction, and a searchable
//! index format for fast approximate set membership queries across skill docs.

// These modules are wired into the binary once the index lifecycle and CLI
// search layers are added. Until then, their public items are unused from the
// binary's perspective but exercised fully by the module's own tests.
#[allow(dead_code)]
pub(crate) mod index;
#[allow(dead_code)]
pub(crate) mod tokenize;
#[allow(dead_code)]
pub(crate) mod xor;
