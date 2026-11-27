use syn::{File, Item, ItemFn, ItemStruct, parse_quote};

use super::{Declaration, expand};

fn adapter() -> ItemFn {
    parse_quote! {
        fn f(params: &FArgs) {}
    }
}

#[test]
fn rejects_invalid_options() {
    for options in [
        "bad_arg, bad_arg",
        "reject_keywords, reject_keywords",
        r#"owner = "Counter", owner = "dict""#,
        r#"kwarg_error_name = "first", kwarg_error_name = "second""#,
    ] {
        let result = syn::parse_str::<Declaration>(&format!(r#""f(a, b)", {options}"#));
        assert_eq!(
            result.err().expect(options).to_string(),
            "duplicate py_signature option"
        );
    }
    let result = syn::parse_str::<Declaration>(r#""f()", style = unpack"#);
    assert_eq!(
        result.err().expect("removed style option").to_string(),
        "unknown py_signature option"
    );
    let result = syn::parse_str::<Declaration>(r#""f(x)", bind = borrowed"#);
    assert_eq!(result.err().unwrap().to_string(), "unknown py_signature option");
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
        (
            "f(self: str, /, x)",
            "a bound receiver cannot have an annotation or a default",
        ),
        (
            "f(self=None, /)",
            "a bound receiver cannot have an annotation or a default",
        ),
        ("f(x: str = 1)", "unsupported default for this parameter type"),
        ("f(x = 1e999)", "unsupported default for this parameter type"),
        ("f(x: SupportsIndex = 0.5)", "expected an integer literal"),
        ("f(x: SupportsIndex = None)", "expected an integer literal"),
        (
            "f(x: i64 = -9223372036854775809)",
            "expected a signed 64-bit integer literal",
        ),
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
    let function = parse_quote!(
        fn f(items: &FArgs) {}
    );
    assert_eq!(
        expand(&declaration, function).unwrap_err().to_string(),
        "expected `params` to name the bound argument struct",
    );
    let declaration = parse_quote!("f()", owner = "Counter");
    assert_eq!(
        expand(&declaration, adapter()).unwrap_err().to_string(),
        "owner requires a positional-only self parameter"
    );
    let declaration = parse_quote!("f(x, /)");
    let function = parse_quote!(
        fn f(params: &(Value,)) {}
    );
    assert_eq!(
        expand(&declaration, function).unwrap_err().to_string(),
        "expected the name of the bound argument type"
    );
}

#[test]
fn generates_bound_fields_in_declaration_order() {
    let declaration = parse_quote!(
        "sample_fn(self, label: str, omitted=..., /, scale=1.5, *items, sep: None | str=None, limit: SupportsIndex=-1, ratio: float=0.5, end=' ', flag=False, other=None, **extras)",
        owner = "Example"
    );
    let function = parse_quote!(
        fn sample(params: &mut SampleFnArgs) {}
    );
    let expanded: ItemFn = syn::parse2(expand(&declaration, function).unwrap()).unwrap();
    let expected: ItemStruct = parse_quote! {
        struct SampleFnArgsFields {
            label: crate::args::StrArg,
            omitted: Option<crate::value::Value>,
            scale: crate::value::Value,
            items: Vec<crate::value::Value>,
            sep: Option<crate::args::StrArg>,
            limit: crate::args::IntArg,
            ratio: f64,
            end: crate::args::DefaultedStringArg,
            flag: crate::value::Value,
            other: crate::value::Value,
            extras: crate::args::KwargsValues,
        }
    };
    assert!(expanded.block.stmts.contains(&syn::Stmt::Item(Item::Struct(expected))));
}

#[test]
fn one_arg_uses_the_declared_name_for_errors() {
    let declaration = parse_quote!("f(x, /)", error_name = "module.f");
    let output = expand(&declaration, adapter()).unwrap().to_string();
    assert!(output.contains(r#""module.f""#));
}

#[test]
fn named_struct_binders_allow_rust_parameters_alongside_args() {
    let signature = "f(x: float, /)";
    for (declaration, function) in [
        (parse_quote!(#signature), adapter()),
        (parse_quote!("f(args, /)"), adapter()),
        (
            parse_quote!("f(vm, /)"),
            parse_quote!(
                fn f(vm: &mut VM<'_>, params: &FArgs) {}
            ),
        ),
        (
            parse_quote!("f(x, /)"),
            parse_quote!(
                fn f(x: State, params: &FArgs, vm: &mut VM<'_>) {}
            ),
        ),
    ] {
        let output: ItemFn = syn::parse2(expand(&declaration, function).unwrap()).unwrap();
        assert!(
            output
                .block
                .stmts
                .iter()
                .any(|stmt| matches!(stmt, syn::Stmt::Item(Item::Struct(_))))
        );
    }
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
fn empty_named_binder_has_an_inline_fast_path() {
    let binder = super::expand_struct(
        &parse_quote!("f()"),
        parse_quote!(
            struct FArgs;
        ),
    )
    .unwrap();
    assert!(binder.to_string().contains("ArgValues :: Empty"));
}

#[test]
fn a_callable_with_parameters_still_needs_a_binder() {
    for signature in ["f(x, /)", "f(x)", "f(*items)", "f(*, x=None)", "f(self, /, **kw)"] {
        let declaration = parse_quote!(#signature);
        let function = parse_quote!(
            fn f(vm: &mut VM<'_>) {}
        );
        assert_eq!(
            expand(&declaration, function).unwrap_err().to_string(),
            "expected `params` to name the bound argument struct",
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
        (r#""(x)""#, "a parameter list requires error_name"),
        (
            r#""f(self, /)", owner = "T", error_name = "g""#,
            "choose either owner or error_name",
        ),
        (r#""f(x)", convert(other = T)"#, "unknown converter parameter"),
        (r#""f(x=None)", convert(x = T)"#, "unsupported custom converter default"),
        (r#""f(x: c_int=2147483648)""#, "expected a C int literal"),
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
fn two_args_use_the_inline_storage() {
    let declaration = parse_quote!("divmod(x, y, /)");
    let function = parse_quote!(
        fn divmod(vm: &mut VM<'_>, params: &DivmodArgs) {}
    );
    let output = expand(&declaration, function).unwrap().to_string();
    assert!(output.contains("ArgValues :: Two"));
    assert!(output.contains("defer_drop_mut !"));
}

#[test]
fn rejects_direct_arguments() {
    for function in [
        parse_quote!(
            fn f(x: &Value) {}
        ),
        parse_quote!(
            fn f(x: f64) {}
        ),
        parse_quote!(
            fn f(params: (&Value, &Value)) {}
        ),
        parse_quote!(
            fn f(positional: &ArgPosIter) {}
        ),
    ] {
        assert!(expand(&parse_quote!("f(x, /)"), function).is_err());
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
        assert!(expanded.contains("Option < crate :: args :: IntArg >"), "{expanded}");
    }
}

#[test]
fn bare_variadic_arguments_preserve_inline_storage() {
    let declaration = parse_quote!("group(self, /, *groups)", owner = "re.Match");
    let output = expand(&declaration, adapter()).unwrap().to_string();
    assert!(output.contains("groups : crate :: args :: ArgPosIter"));
    assert!(output.contains(r#"into_pos_only ("re.Match.group""#));
}

#[test]
fn variadic_signatures_with_other_parameters_use_the_general_binder() {
    for signature in [
        "f(x, /, *items)",
        "f(x: str, /, *items)",
        "f(x=None, /, *items)",
        "f(x, *items)",
        "f(*items, x=None)",
        "f(*items, **extra)",
    ] {
        let output = expand(&parse_quote!(#signature), adapter()).unwrap().to_string();
        assert!(
            output.contains("items : Vec < crate :: value :: Value >"),
            "{signature}: expected a variadic collection"
        );
        assert!(
            !output.contains("into_pos_only"),
            "{signature}: used the inline iterator"
        );
    }
}

#[test]
fn reject_keywords_selects_the_positional_helper() {
    for (signature, helper) in [
        ("f()", "check_zero_args"),
        ("f(x=..., /)", "get_zero_one_arg"),
        ("f(x, /)", "get_one_arg"),
        ("f(x, y=..., /)", "get_one_two_args"),
        ("f(x, y, /)", "get_two_args"),
    ] {
        let declaration = parse_quote!(#signature, error_name = "module.f", reject_keywords);
        let output = expand(&declaration, adapter()).unwrap().to_string();
        assert!(
            output.contains(r#"let args = args . reject_kwargs ("module.f" , vm . heap) ? ; let "#),
            "{signature}: {output}"
        );
        assert!(
            output.contains(&format!(r#"args . {helper} ("module.f" , vm . heap)"#)),
            "{signature}: {output}"
        );
    }
    for signature in ["f(x: float, /)", "f(*items)", "f(x, y, z, /)"] {
        let declaration = parse_quote!(#signature, reject_keywords);
        assert_eq!(
            expand(&declaration, adapter()).unwrap_err().to_string(),
            "reject_keywords requires a fixed raw positional signature",
            "{signature}"
        );
    }
}

#[test]
fn borrowed_handlers_share_the_one_argument_fast_path() {
    for function in [
        adapter(),
        parse_quote!(
            fn f(params: &mut FArgs) {}
        ),
    ] {
        let output = expand(&parse_quote!("f(x, /)"), function).unwrap().to_string();
        assert!(output.contains("ArgValues :: One"));
        assert!(output.contains("defer_drop_mut !"));
    }
    let owned = parse_quote!(
        fn f(params: FArgs) {}
    );
    assert_eq!(
        expand(&parse_quote!("f(x)"), owned).unwrap_err().to_string(),
        "expected &Args or &mut Args"
    );
}
