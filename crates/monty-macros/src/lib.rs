//! Procedural macros for the `monty` Python interpreter.
//!
//! - `#[derive(FromArgs)]` — `ArgValues` → typed struct (positional/kwarg
//!   dispatch, defaults, type coercion via `FromValue`, refcount cleanup).
//! - `#[derive(ToArgs)]` — typed struct → host call arguments.
//! - `#[pyattrs]` — native methods → builtin attribute tables and adapters.
//!
//! Generated code is internal to `monty` (`FromArgs`, `pyattrs`) and `monty-types` (`ToArgs`).
//! See the crate `README.md` for usage and the docstrings on `StructAttrs` /
//! `FieldKind` in `from_args.rs` for the full attribute surface.

use proc_macro::TokenStream;

mod from_args;
mod pyattrs;
mod to_args;

/// Collects Python attributes from an implementation of Monty's internal `PyAttrs` trait.
/// Functions retain their explicit native receiver and argument signatures.
#[proc_macro_attribute]
pub fn pyattrs(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(proc_macro2::Span::call_site(), "#[pyattrs] does not take arguments")
            .into_compile_error()
            .into();
    }
    let input = syn::parse_macro_input!(input as syn::ItemImpl);
    pyattrs::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

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

/// Projects a struct into host call arguments using `#[from_args(...)]` attributes.
/// `varargs` fields contribute one positional argument per element.
#[proc_macro_derive(ToArgs, attributes(from_args))]
pub fn derive_to_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    to_args::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
