use syn::{File, Item, ItemFn, ItemStruct, parse_quote};

use super::{Declaration, expand};

fn adapter() -> ItemFn {
    parse_quote! {
        fn f(args: &FArgs) {}
    }
}

#[test]
fn rejects_invalid_options() {
    for options in [
        "style = def, style = clinic",
        r#"owner = "Counter", owner = "dict""#,
        r#"kwarg_error_name = "first", kwarg_error_name = "second""#,
    ] {
        let result = syn::parse_str::<Declaration>(&format!(r#""f(a, b)", {options}"#));
        assert_eq!(
            result.err().expect(options).to_string(),
            "duplicate py_signature option"
        );
    }
    let result = syn::parse_str::<Declaration>(r#""f()", unknown = def"#);
    assert_eq!(
        result.err().expect("unknown option").to_string(),
        "unknown py_signature option"
    );
}

#[test]
fn def_rejects_parameter_annotations() {
    for signature in ["f(x: str, /)", "f(x: float)", "f(*, x: SupportsIndex = -1)"] {
        let declaration = syn::parse_str::<Declaration>(&format!(r#""{signature}", style = def"#)).unwrap();
        let error = expand(&declaration, adapter()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "leave parameters unannotated with style = def and convert values in the function body"
        );
    }
}

#[test]
fn rejects_invalid_signatures() {
    for (signature, expected) in [
        ("f(): pass\ndef g()", "expected one Python function signature"),
        ("f(): return #", "expected a Python function signature without a body"),
        ("f(*items: str)", "typed variadic parameters are unsupported"),
        ("f(**extras: str)", "typed variadic parameters are unsupported"),
        ("f(mut)", "parameter name must also be a Rust identifier"),
        ("f(x: list[str])", "unsupported native parameter annotation"),
        ("f(x: bytes)", "unsupported native parameter annotation"),
        ("f(x: str = 1)", "unsupported default for this parameter type"),
        ("f(x = 1e999)", "unsupported default for this parameter type"),
        ("f(x: SupportsIndex = 0.5)", "expected an i64 literal for SupportsIndex"),
        (
            "f(x: SupportsIndex = None)",
            "expected an i64 literal for SupportsIndex",
        ),
        (
            "f(x: SupportsIndex = 9223372036854775808)",
            "expected an i64 literal for SupportsIndex",
        ),
    ] {
        let declaration = parse_quote!(#signature);
        let error = expand(&declaration, adapter()).unwrap_err();
        assert_eq!(error.to_string(), expected, "{signature}");
    }
}

#[test]
fn rejects_incompatible_rust_adapters() {
    let declaration = parse_quote!("f()");
    for (function, expected) in [
        (
            parse_quote!(
                fn f(n: usize, items: &[Value]) {}
            ),
            "expected a parameter of type &FArgs",
        ),
        (
            parse_quote!(
                fn f(_: &FArgs) {}
            ),
            "expected a name for the bound arguments",
        ),
    ] {
        assert_eq!(expand(&declaration, function).unwrap_err().to_string(), expected);
    }
    let declaration = parse_quote!("f()", owner = "Counter");
    assert_eq!(
        expand(&declaration, adapter()).unwrap_err().to_string(),
        "owner requires a positional-only self parameter"
    );
}

#[test]
fn generates_bound_fields_in_declaration_order() {
    let declaration = parse_quote!(
        "sample_fn(self, label: str, omitted=..., /, scale=1.5, *items, sep: None | str=None, limit: SupportsIndex=-1, ratio: float=0.5, end=' ', flag=False, other=None, **extras)",
        owner = "Example"
    );
    let function = parse_quote!(
        fn sample(args: &mut SampleFnArgs) {}
    );
    let expanded: File = syn::parse2(expand(&declaration, function).unwrap()).unwrap();
    let expected: ItemStruct = parse_quote! {
        struct SampleFnArgs {
            label: crate::args::StrArg,
            omitted: Option<crate::value::Value>,
            scale: crate::value::Value,
            items: Vec<crate::value::Value>,
            sep: Option<crate::args::StrArg>,
            limit: i64,
            ratio: f64,
            end: crate::args::DefaultedStringArg,
            flag: crate::value::Value,
            other: crate::value::Value,
            extras: crate::args::KwargsValues,
        }
    };
    assert!(expanded.items.contains(&Item::Struct(expected)));
}
