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
        "bad_arg, bad_arg",
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
        ("f(x: SupportsIndex = 0.5)", "expected an integer literal"),
        ("f(x: SupportsIndex = None)", "expected an integer literal"),
        (
            "f(x: i64 = 9223372036854775808)",
            "expected a signed 64-bit integer literal",
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
            limit: crate::types::LongInt,
            ratio: f64,
            end: crate::args::DefaultedStringArg,
            flag: crate::value::Value,
            other: crate::value::Value,
            extras: crate::args::KwargsValues,
        }
    };
    assert!(expanded.items.contains(&Item::Struct(expected)));
}

#[test]
fn one_arg_requires_one_positional_only_argument() {
    for signature in ["f()", "f(x)", "f(x=1.0, /)", "f(x, y, /)", "f(x, /, **kwargs)"] {
        let declaration = parse_quote!(#signature, style = one_arg);
        let error = expand(&declaration, adapter()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "`style = one_arg` requires one positional-only parameter without a default"
        );
    }
}

#[test]
fn one_arg_checks_the_rust_argument() {
    let declaration = parse_quote!("f(x, /)", style = one_arg);
    let function = parse_quote!(
        fn f(x: Value) {}
    );
    assert_eq!(
        expand(&declaration, function).unwrap_err().to_string(),
        "expected &Value for a single positional argument"
    );
    assert_eq!(
        expand(&declaration, adapter()).unwrap_err().to_string(),
        "expected a parameter named `x` of type &Value"
    );
}

#[test]
fn one_arg_uses_the_declared_name_for_errors() {
    for options in [r#"owner = "T""#, r#"kwarg_error_name = "g""#] {
        let declaration = syn::parse_str::<Declaration>(&format!(r#""f(x, /)", style = one_arg, {options}"#)).unwrap();
        assert_eq!(
            expand(&declaration, adapter()).unwrap_err().to_string(),
            "`style = one_arg` uses the callable name for errors"
        );
    }
}

#[test]
fn one_arg_uses_the_existing_positional_fast_path() {
    let declaration = parse_quote!("len(obj, /)", style = one_arg);
    let function = parse_quote!(
        fn builtin_len(vm: &mut VM<'_>, obj: &Value) {
            obj.py_len(vm)
        }
    );
    let expanded: ItemFn = syn::parse2(expand(&declaration, function).unwrap()).unwrap();
    let expected: ItemFn = parse_quote! {
        fn builtin_len(vm: &mut VM<'_>, obj: crate::args::ArgValues) {
            let obj = obj.get_one_arg("len", vm.heap)?;
            crate::defer_drop!(obj, vm);
            { obj.py_len(vm) }
        }
    };
    assert_eq!(expanded, expected);
}

#[test]
fn no_args_checks_before_running_the_method() {
    let declaration = parse_quote!("keys(self, /)", owner = "dict");
    let function = parse_quote!(
        fn keys(&self, vm: &mut VM<'_>) {
            self.make_view(vm)
        }
    );
    let expanded: ItemFn = syn::parse2(expand(&declaration, function).unwrap()).unwrap();
    let expected: ItemFn = parse_quote! {
        fn keys(&self, vm: &mut VM<'_>, __py_args: crate::args::ArgValues) {
            __py_args.check_zero_args("dict.keys", vm.heap)?;
            self.make_view(vm)
        }
    };
    assert_eq!(expanded, expected);
}

#[test]
fn infers_no_arguments_for_a_free_function() {
    let f = parse_quote!(
        fn f(vm: &mut VM<'_>) {}
    );
    let expanded: ItemFn = syn::parse2(expand(&parse_quote!("f()"), f).unwrap()).unwrap();
    let expected = parse_quote! {
        fn f(vm: &mut VM<'_>, __py_args: crate::args::ArgValues) {
            __py_args.check_zero_args("f", vm.heap)?;
        }
    };
    assert_eq!(expanded, expected);
}

#[test]
fn infers_the_same_check_for_a_named_binder() {
    let binder = super::expand_struct(
        &parse_quote!("f()"),
        parse_quote!(
            struct FArgs;
        ),
    )
    .unwrap();
    assert!(binder.to_string().contains("check_zero_args"));
    let python = super::expand_struct(
        &parse_quote!("f()", style = def),
        parse_quote!(
            struct FArgs;
        ),
    )
    .unwrap();
    assert!(!python.to_string().contains("check_zero_args"));
}

#[test]
fn a_callable_with_parameters_still_needs_a_binder() {
    for signature in [
        "f(x, /)",
        "f(x)",
        "f(self=None, /)",
        "f(self: str, /)",
        "f(*items)",
        "f(*, x=None)",
        "f(self, /, **kw)",
    ] {
        let declaration = parse_quote!(#signature);
        let function = parse_quote!(
            fn f(vm: &mut VM<'_>) {}
        );
        assert_eq!(
            expand(&declaration, function).unwrap_err().to_string(),
            "expected a parameter of type &FArgs",
            "{signature}"
        );
    }
}

#[test]
fn named_binder_preserves_visibility_and_defaults() {
    let declaration = parse_quote!(
        "(x: c_int = -1, y=..., *, truth: bool=False)",
        error_name = "sample",
        convert(y = Part)
    );
    let generated = super::expand_struct(
        &declaration,
        parse_quote!(
            pub(crate) struct Sample;
        ),
    )
    .unwrap();
    let expanded: File = syn::parse2(generated).unwrap();
    let expected: ItemStruct = parse_quote! {
        pub(crate) struct Sample {
            pub(crate) x: i32,
            pub(crate) y: Option<Part>,
            pub(crate) truth: crate::args::LaxBool,
        }
    };
    assert!(expanded.items.contains(&Item::Struct(expected)));
}

#[test]
fn rejects_invalid_named_binders_and_converters() {
    let declaration = parse_quote!("f()");
    for item in [
        parse_quote!(
            struct F(u32);
        ),
        parse_quote!(
            struct F<T>;
        ),
    ] {
        assert_eq!(
            super::expand_struct(&declaration, item).unwrap_err().to_string(),
            "expected a unit struct without generics"
        );
    }
    for (options, expected) in [
        (
            r#""f(*items)", style = unpack"#,
            "`style = unpack` requires a fixed positional range",
        ),
        (
            r#""f(**extras)", style = parse_tuple"#,
            "`style = parse_tuple` rejects all keyword arguments",
        ),
        (r#""(x)""#, "a parameter list requires error_name"),
        (
            r#""f(self, /)", owner = "T", error_name = "g""#,
            "choose either owner or error_name",
        ),
        (r#""f(x)", convert(other = T)"#, "unknown converter parameter"),
        (
            r#""f(x: int)", convert(x = T)"#,
            "a custom converter supplies the parameter type",
        ),
        (r#""f(x=None)", convert(x = T)"#, "unsupported custom converter default"),
        (r#""f(x: c_int=2147483648)""#, "expected a C int literal"),
        (
            r#""f(x, /)", style = one_arg, bad_arg"#,
            "fixed-arity helpers do not accept parser modifiers or custom converters",
        ),
        (
            r#""f()", owner = "T""#,
            "owner requires a positional-only self parameter",
        ),
    ] {
        let declaration = syn::parse_str::<Declaration>(options).unwrap();
        assert_eq!(expand(&declaration, adapter()).unwrap_err().to_string(), expected);
    }
    let duplicate = syn::parse_str::<Declaration>(r#""f(x)", convert(x = T, x = U)"#);
    assert_eq!(duplicate.err().unwrap().to_string(), "duplicate converter");
}

#[test]
fn destructured_adapter_checks_the_argument_type() {
    let declaration = parse_quote!("f(x)");
    let function = parse_quote!(
        fn f(FArgs { x }: &GArgs) {}
    );
    assert_eq!(
        expand(&declaration, function).unwrap_err().to_string(),
        "expected the destructured argument type"
    );
}

#[test]
fn two_args_requires_an_ordered_pair_of_raw_values() {
    let function: ItemFn = parse_quote!(
        fn divmod((x, y): (&Value, &Value)) {}
    );
    for (sig, expected) in [
        (
            "divmod(x, y)",
            "`style = two_args` requires two positional-only parameters",
        ),
        (
            "divmod(x, y=0, /)",
            "`style = two_args` requires two raw values and uses the callable name for errors",
        ),
        (
            "divmod(y, x, /)",
            "expected a pair of &Value parameters in signature order",
        ),
    ] {
        let declaration = parse_quote!(#sig, style = two_args);
        assert_eq!(
            expand(&declaration, function.clone()).unwrap_err().to_string(),
            expected
        );
    }
}

#[test]
fn full_width_integer_defaults_preserve_magnitude_and_sign() {
    for (literal, decimal) in [
        ("0X1_0000_0000_0000_0000", "18446744073709551616"),
        ("-18446744073709551617", "-18446744073709551617"),
    ] {
        let signature = format!("f(x: SupportsIndex={literal}, *, default: int={literal}, y: SupportsIndex=...)");
        let declaration = parse_quote!(#signature);
        let expanded = expand(&declaration, adapter()).unwrap().to_string();
        assert!(expanded.contains(decimal), "{expanded}");
        assert!(expanded.contains("Option < crate :: types :: LongInt >"), "{expanded}");
    }
}
