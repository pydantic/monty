//! Helpers shared by more than one integration test.
//!
//! Each `tests/*.rs` is its own crate, so anything two of them need lives here
//! and is pulled in with `mod common;`.

pub mod dump_corpus;
