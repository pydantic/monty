//! Rendering checks against hand-built rustdoc JSON crates, so they run
//! without the nightly toolchain that generation itself needs.

use std::collections::HashMap;

use insta::assert_snapshot;
use monty_apidoc::{docs_md::process_docs, sig::SigCtx, symbols::SymbolMap};
use rustdoc_types::{
    Abi, Crate, Enum, FORMAT_VERSION, Function, FunctionHeader, FunctionSignature, GenericParamDef,
    GenericParamDefKind, Generics, Id, Item, ItemEnum, Module, Path, Struct, StructKind, Target, Trait, Type, Variant,
    VariantKind, Visibility,
};

/// A crate whose root module (id 0) contains `root` out of `items`, all
/// public; the rest are reachable only through their parents.
fn crate_of(root: Vec<Id>, items: Vec<Item>) -> Crate {
    let mut index: HashMap<Id, Item> = items.into_iter().map(|item| (item.id, item)).collect();
    index.insert(Id(0), item(0, "root", module(root)));
    Crate {
        root: Id(0),
        crate_version: None,
        includes_private: false,
        index,
        paths: HashMap::new(),
        external_crates: HashMap::new(),
        target: Target {
            triple: String::new(),
            target_features: Vec::new(),
        },
        format_version: FORMAT_VERSION,
    }
}

fn item(id: u32, name: &str, inner: ItemEnum) -> Item {
    Item {
        id: Id(id),
        crate_id: 0,
        name: Some(name.to_owned()),
        span: None,
        visibility: Visibility::Public,
        docs: None,
        links: HashMap::new(),
        attrs: Vec::new(),
        deprecation: None,
        stability: None,
        const_stability: None,
        inner,
    }
}

fn module(items: Vec<Id>) -> ItemEnum {
    ItemEnum::Module(Module {
        is_crate: false,
        items,
        is_stripped: false,
    })
}

fn generics() -> Generics {
    Generics {
        params: Vec::new(),
        where_predicates: Vec::new(),
    }
}

fn unit_struct() -> ItemEnum {
    ItemEnum::Struct(Struct {
        kind: StructKind::Unit,
        generics: generics(),
        impls: Vec::new(),
    })
}

fn function(abi: Abi) -> ItemEnum {
    ItemEnum::Function(Function {
        sig: FunctionSignature {
            inputs: Vec::new(),
            output: None,
            is_c_variadic: false,
        },
        generics: generics(),
        header: FunctionHeader {
            is_const: false,
            is_unsafe: false,
            is_async: false,
            abi,
        },
        has_body: true,
        default_unstable: None,
    })
}

/// Renders `docs` with no resolvable links.
fn docs(text: &str) -> String {
    let krate = crate_of(Vec::new(), Vec::new());
    let symbols = SymbolMap::build(&[]);
    process_docs(text, 2, &HashMap::new(), "k", &krate, &symbols)
}

#[test]
fn four_backtick_fence_keeps_embedded_fence() {
    let text = "Example:\n\n````markdown\n```rust\nlet x = 1;\n```\n````\n\nAfter.";
    assert_snapshot!(docs(text), @r"
    Example:

    ````markdown
    ```rust
    let x = 1;
    ```
    ````

    After.
    ");
}

#[test]
fn tilde_fence_closes_only_on_tildes() {
    let text = "~~~\n```\nnot a close\n~~~\ndone";
    assert_snapshot!(docs(text), @r"
    ~~~rust
    ```
    not a close
    ~~~
    done
    ");
}

#[test]
fn rust_path_definition_with_title_is_dropped() {
    let text = "See [`Pool`] and [`Pool`][].\n\n[`Pool`]: crate::pool::Pool \"The pool\"";
    assert_eq!(docs(text), "See `Pool` and `Pool`.\n");
}

#[test]
fn module_relative_link_text_is_trimmed() {
    assert_eq!(
        docs("See [`super::MountMode::OverlayMemory`](super::MountMode::OverlayMemory)."),
        "See `MountMode::OverlayMemory`."
    );
}

#[test]
fn definition_with_url_target_is_kept() {
    let text = "See [docs].\n\n[docs]: https://example.com \"Title\"";
    assert_eq!(docs(text), text);
}

#[test]
fn relative_paths_collapse_to_the_name() {
    let krate = crate_of(Vec::new(), Vec::new());
    let symbols = SymbolMap::build(&[]);
    let ctx = SigCtx {
        krate: &krate,
        symbols: &symbols,
        rustdoc_name: "k",
    };
    for (written, expected) in [
        ("super::error::MountError", "MountError"),
        ("self::Local", "Local"),
        ("crate::inner::Deep", "Deep"),
        ("std::io::Error", "std::io::Error"),
    ] {
        let path = Path {
            path: written.to_owned(),
            id: Id(99),
            args: None,
        };
        assert_eq!(ctx.path_str(&path), expected);
    }
}

#[test]
fn nested_modules_are_indexed_to_their_own_anchor() {
    // root → pb → exc_data → Kind, mirroring `monty_proto::pb::exc_data::Kind`
    let krate = crate_of(
        vec![Id(1)],
        vec![
            item(1, "pb", module(vec![Id(2)])),
            item(2, "exc_data", module(vec![Id(3)])),
            item(3, "Kind", unit_struct()),
        ],
    );
    let symbols = SymbolMap::build(&[("k", &krate)]);
    assert_eq!(symbols.resolve("k", &krate, Id(3)).as_deref(), Some("#kind"));
    assert_eq!(symbols.resolve("k", &krate, Id(2)).as_deref(), Some("#exc_data"));
}

#[test]
fn variant_fields_anchor_to_the_enum() {
    let krate = crate_of(
        vec![Id(1)],
        vec![
            item(
                1,
                "Kind",
                ItemEnum::Enum(Enum {
                    generics: generics(),
                    has_stripped_variants: false,
                    variants: vec![Id(2), Id(4)],
                    impls: Vec::new(),
                }),
            ),
            item(
                2,
                "Named",
                ItemEnum::Variant(Variant {
                    kind: VariantKind::Struct {
                        fields: vec![Id(3)],
                        has_stripped_fields: false,
                    },
                    discriminant: None,
                }),
            ),
            item(3, "field", ItemEnum::StructField(Type::Primitive("u8".to_owned()))),
            item(
                4,
                "Tuple",
                ItemEnum::Variant(Variant {
                    kind: VariantKind::Tuple(vec![Some(Id(5)), None]),
                    discriminant: None,
                }),
            ),
            item(5, "0", ItemEnum::StructField(Type::Primitive("u8".to_owned()))),
        ],
    );
    let symbols = SymbolMap::build(&[("k", &krate)]);
    assert_eq!(symbols.resolve("k", &krate, Id(3)).as_deref(), Some("#kind"));
    assert_eq!(symbols.resolve("k", &krate, Id(5)).as_deref(), Some("#kind"));
}

#[test]
fn extern_abis_are_spelled_as_written() {
    let krate = crate_of(Vec::new(), Vec::new());
    let symbols = SymbolMap::build(&[]);
    let ctx = SigCtx {
        krate: &krate,
        symbols: &symbols,
        rustdoc_name: "k",
    };
    let decl = |abi: Abi| match function(abi) {
        ItemEnum::Function(f) => ctx.fn_decl("f", &f, ""),
        _ => unreachable!(),
    };
    assert_eq!(decl(Abi::Rust), "pub fn f()");
    assert_eq!(decl(Abi::C { unwind: false }), "pub extern \"C\" fn f()");
    assert_eq!(decl(Abi::C { unwind: true }), "pub extern \"C-unwind\" fn f()");
    assert_eq!(decl(Abi::System { unwind: false }), "pub extern \"system\" fn f()");
    assert_eq!(decl(Abi::Other("efiapi".to_owned())), "pub extern \"efiapi\" fn f()");
}

#[test]
fn generic_associated_types_keep_their_params() {
    let krate = crate_of(
        vec![Id(1)],
        vec![
            item(
                1,
                "Lend",
                ItemEnum::Trait(Trait {
                    is_auto: false,
                    is_unsafe: false,
                    is_dyn_compatible: false,
                    items: vec![Id(2)],
                    generics: generics(),
                    bounds: Vec::new(),
                    implementations: Vec::new(),
                }),
            ),
            item(
                2,
                "Item",
                ItemEnum::AssocType {
                    generics: Generics {
                        params: vec![GenericParamDef {
                            name: "'a".to_owned(),
                            kind: GenericParamDefKind::Lifetime { outlives: Vec::new() },
                        }],
                        where_predicates: vec![rustdoc_types::WherePredicate::LifetimePredicate {
                            lifetime: "Self".to_owned(),
                            outlives: vec!["'a".to_owned()],
                        }],
                    },
                    bounds: Vec::new(),
                    type_: None,
                    default_unstable: None,
                },
            ),
        ],
    );
    let symbols = SymbolMap::build(&[("k", &krate)]);
    let ctx = SigCtx {
        krate: &krate,
        symbols: &symbols,
        rustdoc_name: "k",
    };
    assert_snapshot!(ctx.item_decl("Lend", &krate.index[&Id(1)]), @r"
    pub trait Lend {
        type Item<'a> where Self: 'a;
    }
    ");
}
