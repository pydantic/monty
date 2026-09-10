//! Renders rustdoc JSON into the markdown API reference pages under
//! `docs/api/rust/`.
//!
//! The binary (`main.rs`) runs the pinned nightly rustdoc and feeds each
//! crate's JSON through [`render::render_page`]; the rendering lives in this
//! library so `tests/` can exercise it against hand-built [`rustdoc_types`]
//! crates without a toolchain.

// an internal, unpublished tool: unhandled rustdoc shapes panic on purpose
// (see `sig`), and the public-API pedantry buys nothing here
#![expect(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::implicit_hasher,
    reason = "internal tool crate; panics on unhandled rustdoc shapes are the intended failure mode"
)]

pub mod docs_md;
pub mod render;
pub mod sig;
pub mod symbols;

/// One crate page. Most of the crates use
/// `#![doc = include_str!("../README.md")]` as crate docs, which would
/// duplicate install instructions and crates.io links into the reference —
/// so every page opens with a short hand-written `intro` instead, and
/// `render_crate_docs` includes the crate docs only where they are genuine
/// `//!` module documentation (`monty-fs`).
pub struct CrateConfig {
    pub name: &'static str,
    pub intro: &'static str,
    pub render_crate_docs: bool,
    /// Explicit reading order for root items: listed names come first, in
    /// this order; everything else keeps source order. A name not found at
    /// the crate root is a generation error (catches renames).
    pub order: &'static [&'static str],
    /// Cargo features enabled when documenting, so feature-gated public
    /// items appear; the page notes them. Test-only features stay off.
    pub features: &'static [&'static str],
}
