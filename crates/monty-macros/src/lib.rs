//! Procedural macros for the `monty` Python interpreter.
//!
//! - `#[derive(FromArgs)]` — `ArgValues` → typed struct (positional/kwarg
//!   dispatch, defaults, type coercion via `FromValue`, refcount cleanup).
//! - `#[derive(ToArgs)]` — typed struct → host call arguments.
//! - `#[explicit_discriminants(checksum = ...)]` — pin permanent numeric codes on enum variants.
//!
//! Argument-conversion code is internal to `monty` (`FromArgs`) and `monty-types` (`ToArgs`).
//! See the crate `README.md` for usage and the docstrings on `StructAttrs` /
//! `FieldKind` in `from_args.rs` for the full attribute surface.

use proc_macro::TokenStream;

mod explicit_discriminants;
mod from_args;
#[cfg(feature = "native-signatures")]
mod py_signature;
mod to_args;

/// Requires distinct explicit integer identity codes and a matching `checksum` argument.
/// Place before derives to check every declared variant, regardless of `cfg` conditions.
/// Missing or mismatched checksums report a copyable attribute; review changes before updating it.
/// Existing codes must never change or be reused. The enum is returned unchanged.
#[proc_macro_attribute]
pub fn explicit_discriminants(args: TokenStream, input: TokenStream) -> TokenStream {
    explicit_discriminants::expand(args.into(), input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Generates a native call adapter from a Python signature.
#[cfg(feature = "native-signatures")]
#[proc_macro_attribute]
pub fn py_signature(attr: TokenStream, item: TokenStream) -> TokenStream {
    let declaration = syn::parse_macro_input!(attr as py_signature::Declaration);
    let function = syn::parse_macro_input!(item as syn::ItemFn);
    py_signature::expand(&declaration, function)
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
