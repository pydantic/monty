//! Procedural macros for the `monty` Python interpreter.
//!
//! - `#[derive(FromArgs)]` — `ArgValues` → typed struct (positional/kwarg
//!   dispatch, defaults, type coercion via `FromValue`, refcount cleanup).
//! - `#[derive(ToArgs)]` — typed struct → `(Vec<MontyObject>, kwargs)`.
//!
//! Generated code emits `crate::...` paths and only compiles inside `monty`.
//! See the crate `README.md` for usage and the docstrings on `StructAttrs` /
//! `FieldKind` in `from_args.rs` for the full attribute surface.

use proc_macro::TokenStream;

mod from_args;
mod to_args;

/// Derive `FromArgs::from_args`. Each struct field becomes a Python
/// parameter; the generated body parses an `ArgValues` and populates the
/// struct, returning `RunResult<Self>`.
///
/// Field types must implement `monty::args::FromValue`. Fields must appear
/// in Python signature order:
/// `[pos_only…] [pos_or_keyword…] [varargs] [kw_only…] [varkwargs]`, with
/// required fields preceding optional ones in each region.
///
/// `#[from_args(name = "...")]` is required on the struct. The remaining
/// attributes select CPython error-wording style or field roles; see the
/// crate `README.md` and the source docstrings.
#[proc_macro_derive(FromArgs, attributes(from_args))]
pub fn derive_from_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    from_args::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derive `ToArgs::to_args` — projects a struct into
/// `(Vec<MontyObject>, Vec<(MontyObject, MontyObject)>)`. Reuses the
/// `#[from_args(...)]` field attributes (`pos_only`, `kw_only`, `varargs`)
/// so a struct that derives both stays consistent in both directions. Each
/// field type must implement `monty::args::ToMontyObject`.
#[proc_macro_derive(ToArgs, attributes(from_args))]
pub fn derive_to_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    to_args::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
