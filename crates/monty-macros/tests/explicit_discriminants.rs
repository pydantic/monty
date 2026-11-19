//! Identity codes must be explicit and independent of variant declaration order and configuration.

use insta::{allow_duplicates, assert_snapshot};
use monty_macros::explicit_discriminants;
use proc_macro2::TokenStream;
use quote::quote;

#[path = "../src/explicit_discriminants.rs"]
mod expansion;

/// An insertion between existing codes must not renumber either neighbour.
#[explicit_discriminants(checksum = 0x1afb_274d_8a10_ccbd)]
enum Reordered {
    First = 7,
    Inserted = 128,
    Last = 2,
}

/// The checksum includes inactive variants, while subsequent derives must not see their payloads.
#[explicit_discriminants(checksum = 0xbfbd_4f2d_67fb_0bee)]
#[derive(Debug, PartialEq, Eq)]
#[repr(u8)]
enum Conditional {
    First = 7,
    #[cfg(any())]
    Inactive(UndefinedType) = 99,
    #[cfg_attr(test, cfg(any()))]
    IndirectlyInactive(UndefinedType) = 100,
    #[cfg(test)]
    Last = 2,
}

/// The checksum ignores declaration order, literal spelling and variant payloads.
#[test]
fn explicit_codes_do_not_depend_on_order() {
    assert_eq!(Reordered::First as u32, 7);
    assert_eq!(Reordered::Inserted as u32, 128);
    assert_eq!(Reordered::Last as u32, 2);
    let inputs = [
        quote! { enum Explicit { First = 7, Inserted = 128, Last = 2 } },
        quote! { enum Explicit { Last = 0x02, First = 0o7, Inserted = 0b1000_0000 } },
        quote! { enum Explicit { First(u64) = 7, Inserted(String) = 128, Last(bool) = 2 } },
    ];
    for input in inputs {
        let output = expansion::expand(quote!(checksum = 0x1afb_274d_8a10_ccbd), input.clone()).unwrap();
        assert_eq!(output.to_string(), input.to_string());
    }
}

/// Attribute expansion must check all variants before Rust filters them for derives and type checking.
#[test]
fn conditional_variants_share_one_checksum() {
    assert_eq!(Conditional::First as u32, 7);
    assert_eq!(Conditional::Last as u32, 2);
    assert_ne!(Conditional::First, Conditional::Last);
    let input = quote! {
        #[derive(Debug)]
        enum Conditional {
            First = 7,
            #[cfg(any())]
            Inactive(UndefinedType) = 99,
            #[cfg_attr(test, cfg(any()))]
            IndirectlyInactive(UndefinedType) = 100,
            #[cfg(test)]
            Last = 2,
        }
    };
    let output = expansion::expand(quote!(checksum = 0xbfbd_4f2d_67fb_0bee), input.clone()).unwrap();
    assert_eq!(output.to_string(), input.to_string());
}

/// Both implicit and cfg-gated codes must be rejected before configuration can hide them.
#[test]
fn implicit_codes_are_rejected() {
    let inputs = [
        quote! { enum Implicit { First } },
        quote! { enum Implicit { First = 7, Last } },
        quote! { enum Implicit { First = 7, #[cfg(any())] Last } },
    ];
    allow_duplicates! {
        for input in inputs {
            assert_snapshot!(
                expansion::expand(TokenStream::new(), input).unwrap_err().to_string(),
                @"each variant must have an explicit integer discriminant"
            );
        }
    }
}

/// Numeric codes must be unique even when configuration prevents variants from coexisting.
#[test]
fn duplicate_codes_are_rejected() {
    let inputs = [
        quote! { enum Duplicate { First = 7, Last = 0x07 } },
        quote! {
            enum Duplicate {
                #[cfg(test)]
                First = 7,
                #[cfg(not(test))]
                Last = 0x07,
            }
        },
        quote! {
            enum Duplicate {
                #[cfg_attr(test, cfg(any()))]
                First = 7,
                #[cfg_attr(not(test), cfg(any()))]
                Last = 0x07,
            }
        },
    ];
    allow_duplicates! {
        for input in inputs {
            assert_snapshot!(
                expansion::expand(TokenStream::new(), input).unwrap_err().to_string(),
                @"duplicate identity code 7: `First` and `Last`"
            );
        }
    }
}

/// Codes cannot depend on another variant or on a computed expression.
#[test]
fn computed_codes_are_rejected() {
    let input = quote! {
        enum Computed { First = 7, Last = Self::First as isize + 1 }
    };
    assert_snapshot!(
        expansion::expand(TokenStream::new(), input).unwrap_err().to_string(),
        @"each variant must have an explicit integer discriminant"
    );
}

/// A missing checksum reports the full attribute needed to initialize it.
#[test]
fn missing_checksum_reports_current_value() {
    let input = quote! { enum Missing { Only = 7 } };
    assert_snapshot!(
        expansion::expand(TokenStream::new(), input).unwrap_err().to_string(),
        @"
        missing identity-code checksum; use:
        #[monty_macros::explicit_discriminants(checksum = 0xb143_3000_ed8e_a4c0)]
        "
    );
}

/// A stale checksum reports both values and a copyable replacement attribute.
#[test]
fn mismatched_checksum_reports_current_value() {
    let input = quote! { enum Changed { Only = 7 } };
    assert_snapshot!(
        expansion::expand(quote!(checksum = 0), input).unwrap_err().to_string(),
        @"
        identity-code checksum mismatch: expected 0x0000_0000_0000_0000, actual 0xb143_3000_ed8e_a4c0
        review existing codes before updating to:
        #[monty_macros::explicit_discriminants(checksum = 0xb143_3000_ed8e_a4c0)]
        "
    );
}

/// Code changes, renames, additions and removals must each invalidate the baseline.
#[test]
fn changed_identity_mappings_are_rejected() {
    let inputs = [
        quote! { enum Changed { First = 8, Inserted = 128, Last = 2 } },
        quote! { enum Changed { Renamed = 7, Inserted = 128, Last = 2 } },
        quote! { enum Changed { First = 7, Inserted = 128, Last = 2, New = 9 } },
        quote! { enum Changed { First = 7, Last = 2 } },
    ];
    let errors = inputs.map(|input| {
        expansion::expand(quote!(checksum = 0x1afb_274d_8a10_ccbd), input)
            .unwrap_err()
            .to_string()
    });
    assert_snapshot!(errors.join("\n\n"));
}

/// Ambiguous or malformed arguments must not silently disable the check.
#[test]
fn invalid_checksum_arguments_are_rejected() {
    let args = [
        quote!(checksum = 0, checksum = 0),
        quote!(typo = 0),
        quote!(checksum = "0"),
        quote!(checksum = 0x1_0000_0000_0000_0000),
        quote!(checksum),
    ];
    let errors = args.map(|args| {
        expansion::expand(args, quote! { enum Invalid { Only = 7 } })
            .unwrap_err()
            .to_string()
    });
    assert_snapshot!(errors.join("\n\n"));
}

/// Applying the attribute to another data shape must fail rather than skip its check.
#[test]
fn non_enums_are_rejected() {
    let input = quote! { struct NotAnEnum; };
    assert_snapshot!(
        expansion::expand(TokenStream::new(), input).unwrap_err().to_string(),
        @"explicit_discriminants requires an enum"
    );
}
