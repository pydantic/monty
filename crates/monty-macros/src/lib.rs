//! Procedural macros for the `monty` Python interpreter.
//!
//! - `#[derive(FromArgs)]` — `ArgValues` → typed struct.
//! - `#[derive(ToArgs)]` — typed struct → `(Vec<MontyObject>, kwargs)`.
//!
//! See the trait docs in `monty::args` and the per-derive docs below.

use proc_macro::TokenStream;

mod from_args;
mod to_args;

/// Derives `FromArgs::from_args` for a struct, producing the body of a function
/// that consumes an `ArgValues` and populates each field of the struct.
///
/// Replaces hand-written positional/kwarg dispatch code with a declarative
/// struct definition. See `crates/monty/src/args.rs` for the `FromArgs` trait
/// and supported field types via the `FromValue` trait.
///
/// # Struct attributes
///
/// * `#[from_args(name = "function")]` — function name used in error messages
///   (required).
///
/// # Field attributes
///
/// * `#[from_args(default)]` — use `Default::default()` if the argument was not
///   supplied.
/// * `#[from_args(default = <expr>)]` — use `<expr>` if not supplied.
/// * `#[from_args(pos_only)]` — refuse to accept the field as a keyword argument.
/// * `#[from_args(kw_only)]` — refuse to accept the field as a positional
///   argument.
/// * `#[from_args(varargs)]` — collects all extra positional arguments. The
///   field type must be `Vec<T>` where `T: FromValue`.
/// * `#[from_args(varkwargs)]` — collects all unrecognised keyword arguments.
///   The field type must be `KwargsValues`.
/// * `#[from_args(static_string = "Variant")]` — override the auto-derived
///   `StaticStrings::PascalCase(field_ident)` used for kwarg matching.
///
/// # Field ordering
///
/// The macro requires the struct fields to be ordered as in a Python signature:
///
/// ```text
/// [pos_only...] [pos_or_keyword...] [varargs] [kw_only...] [varkwargs]
/// ```
///
/// Within each region, required fields must come before optional ones. The
/// `varargs` field implicitly introduces the `*` separator: any field after it
/// is treated as `kw_only` even without the attribute.
#[proc_macro_derive(FromArgs, attributes(from_args))]
pub fn derive_from_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    from_args::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `ToArgs::to_args` — projects a struct into
/// `(Vec<MontyObject>, Vec<(MontyObject, MontyObject)>)`.
///
/// Reuses `#[from_args(...)]` field attributes (`pos_only`, `kw_only`,
/// `varargs`) so structs that derive both stay consistent in both directions.
/// Each field type must implement `monty::args::ToMontyObject`.
#[proc_macro_derive(ToArgs, attributes(from_args))]
pub fn derive_to_args(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    to_args::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
