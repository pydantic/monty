//! Identity codes must be explicit and independent of variant declaration order.

use insta::{allow_duplicates, assert_snapshot};
use monty_macros::ExplicitDiscriminants;
use syn::{DeriveInput, parse_quote};

#[path = "../src/explicit_discriminants.rs"]
mod expansion;

/// An insertion between existing codes must not renumber either neighbour.
#[derive(ExplicitDiscriminants)]
enum Reordered {
    First = 7,
    Inserted = 128,
    Last = 2,
}

/// The derive accepts nonsequential codes and emits no representation changes.
#[test]
fn explicit_codes_do_not_depend_on_order() {
    assert_eq!(Reordered::First as u32, 7);
    assert_eq!(Reordered::Inserted as u32, 128);
    assert_eq!(Reordered::Last as u32, 2);
    let input = parse_quote! {
        enum Explicit { First = 7, Inserted = 128, Last = 2 }
    };
    assert!(expansion::expand(&input).unwrap().is_empty());
}

/// Both an implicit first code and an implicit successor would make insertion unsafe.
#[test]
fn implicit_codes_are_rejected() {
    let inputs: [DeriveInput; 2] = [
        parse_quote! { enum Implicit { First } },
        parse_quote! { enum Implicit { First = 7, Last } },
    ];
    allow_duplicates! {
        for input in inputs {
            assert_snapshot!(
                expansion::expand(&input).unwrap_err().to_string(),
                @"each variant must have an explicit integer discriminant"
            );
        }
    }
}

/// Codes cannot depend on another variant or on a computed expression.
#[test]
fn computed_codes_are_rejected() {
    let input = parse_quote! {
        enum Computed { First = 7, Last = Self::First as isize + 1 }
    };
    assert_snapshot!(
        expansion::expand(&input).unwrap_err().to_string(),
        @"each variant must have an explicit integer discriminant"
    );
}

/// Applying the derive to another data shape must fail rather than skip its check.
#[test]
fn non_enums_are_rejected() {
    let input = parse_quote! { struct NotAnEnum; };
    assert_snapshot!(
        expansion::expand(&input).unwrap_err().to_string(),
        @"ExplicitDiscriminants requires an enum"
    );
}
