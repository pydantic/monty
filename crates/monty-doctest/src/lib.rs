// mdformat writes the docs' list continuations at four spaces (see `make format-md`),
// which this rustdoc-only lint would reject
#![expect(clippy::doc_overindented_list_items)]
#![doc = include_str!("../../../docs/quickstart/rust.md")]
