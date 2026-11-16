//! Checks identity-bearing enums without tying their codes to declaration order.

use proc_macro2::TokenStream;
use syn::{Data, DeriveInput, Expr, Lit};

/// Rejects implicit or computed discriminants so new variants must choose a permanent code.
pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "ExplicitDiscriminants requires an enum"));
    };
    for variant in &data.variants {
        if !matches!(
            &variant.discriminant,
            Some((_, Expr::Lit(expr))) if matches!(expr.lit, Lit::Int(_))
        ) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "each variant must have an explicit integer discriminant",
            ));
        }
    }
    Ok(TokenStream::new())
}
