//! Checks identity-bearing enums without tying their codes to declaration order.

use std::collections::HashMap;

use proc_macro2::TokenStream;
use syn::{Data, DataEnum, DeriveInput, Expr, ExprLit, Lit, LitInt, meta::parser, parse::Parser};

/// Validates every declared variant, including inactive ones, and returns the item unchanged.
pub(crate) fn expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(item.clone())?;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input,
            "explicit_discriminants requires an enum",
        ));
    };
    let actual = codes_checksum(data)?;
    let expected = expected_checksum(args)?;
    let actual_hex = format_checksum(actual);
    let attribute = format!("#[monty_macros::explicit_discriminants(checksum = {actual_hex})]");
    match expected {
        None => Err(syn::Error::new_spanned(
            &input.ident,
            format!("missing identity-code checksum; use:\n{attribute}"),
        )),
        Some(expected) if expected != actual => Err(syn::Error::new_spanned(
            &input.ident,
            format!(
                "identity-code checksum mismatch: expected {}, actual {actual_hex}\n\
                 review existing codes before updating to:\n{attribute}",
                format_checksum(expected),
            ),
        )),
        Some(_) => Ok(item),
    }
}

/// Checks code uniqueness across all declared variants, then hashes sorted `name:code` pairs.
/// FNV-1a consumes NUL-terminated pairs with codes normalized to decimal.
fn codes_checksum(data: &DataEnum) -> syn::Result<u64> {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;

    let mut codes = Vec::with_capacity(data.variants.len());
    let mut assigned = HashMap::with_capacity(data.variants.len());
    for variant in &data.variants {
        let code = match &variant.discriminant {
            Some((
                _,
                Expr::Lit(ExprLit {
                    lit: Lit::Int(code), ..
                }),
            )) => code.base10_parse::<u64>()?,
            _ => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "each variant must have an explicit integer discriminant",
                ));
            }
        };
        if let Some(first) = assigned.insert(code, &variant.ident) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                format!("duplicate identity code {code}: `{first}` and `{}`", variant.ident),
            ));
        }
        codes.push(format!("{}:{code}", variant.ident));
    }
    codes.sort_unstable();
    let mut hash = OFFSET_BASIS;
    for code in codes {
        for byte in code.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    Ok(hash)
}

/// Reads the checksum argument, rejecting misspelled options and duplicate assignments.
fn expected_checksum(args: TokenStream) -> syn::Result<Option<u64>> {
    let mut checksum = None;
    parser(|meta| {
        if !meta.path.is_ident("checksum") {
            Err(meta.error("expected `checksum = <u64 integer literal>`"))
        } else if checksum.is_some() {
            Err(meta.error("duplicate `checksum`"))
        } else {
            let value: LitInt = meta.value()?.parse()?;
            checksum = Some(value.base10_parse::<u64>()?);
            Ok(())
        }
    })
    .parse2(args)?;
    Ok(checksum)
}

/// Formats a checksum as a fixed-width hex literal suitable for copying into an attribute.
fn format_checksum(checksum: u64) -> String {
    let hex = format!("{checksum:016x}");
    format!("0x{}_{}_{}_{}", &hex[..4], &hex[4..8], &hex[8..12], &hex[12..])
}
