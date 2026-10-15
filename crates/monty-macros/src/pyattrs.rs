use std::{collections::HashSet, mem};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, GenericArgument, GenericParam, ImplItem, ImplItemFn, ItemImpl, PathArguments, ReturnType, Type};

#[derive(Clone, Copy)]
enum Mode {
    MutHeap,
    Heap,
    Immediate,
}

#[derive(Clone, Copy)]
enum Kind {
    Method,
    ClassMethod,
    Value,
}

fn type_path(ty: &Type) -> syn::Result<&syn::PathSegment> {
    match ty {
        Type::Path(path) if path.qself.is_none() => Ok(path.path.segments.last().unwrap()),
        _ => Err(syn::Error::new_spanned(ty, "expected a named type")),
    }
}

fn receiver(ty: &Type) -> syn::Result<(Mode, &Type, syn::Ident)> {
    let marker = type_path(ty)?;
    let mode = match marker.ident.to_string().as_str() {
        "MutHeap" => Mode::MutHeap,
        "Heap" => Mode::Heap,
        "Immediate" => Mode::Immediate,
        _ => {
            return Err(syn::Error::new_spanned(
                ty,
                "Receiver must be MutHeap<T>, Heap<T>, or Immediate<T>",
            ));
        }
    };
    let PathArguments::AngleBracketed(arguments) = &marker.arguments else {
        return Err(syn::Error::new_spanned(ty, "receiver marker requires one native type"));
    };
    let native = match arguments.args.first() {
        Some(GenericArgument::Type(native)) if arguments.args.len() == 1 => native,
        _ => {
            return Err(syn::Error::new_spanned(
                arguments,
                "receiver marker requires one native type",
            ));
        }
    };
    let ident = &type_path(native)?.ident;
    let variant = if matches!(mode, Mode::Immediate) {
        match ident.to_string().as_str() {
            "f64" => format_ident!("Float"),
            "i64" => format_ident!("Int"),
            "bool" => format_ident!("Bool"),
            _ => return Err(syn::Error::new_spanned(native, "Immediate supports f64, i64, or bool")),
        }
    } else {
        ident.clone()
    };
    Ok((mode, native, variant))
}

fn named(ty: &Type, name: &str) -> bool {
    type_path(ty).is_ok_and(|segment| segment.ident == name)
}

fn vm_parameter(ty: &Type) -> bool {
    matches!(ty, Type::Reference(reference) if reference.mutability.is_some() && named(&reference.elem, "VM"))
}

fn validate(method: &ImplItemFn, kind: Kind, mode: Mode, native: &Type) -> syn::Result<()> {
    let signature = &method.sig;
    if signature.constness.is_some()
        || signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || signature
            .generics
            .params
            .iter()
            .any(|param| !matches!(param, GenericParam::Lifetime(_)))
        || signature.generics.where_clause.is_some()
    {
        return Err(syn::Error::new_spanned(
            signature,
            "Python attributes require a safe synchronous function with only lifetime parameters",
        ));
    }
    let parameters = signature
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(arg) => Ok(arg.ty.as_ref()),
            FnArg::Receiver(arg) => Err(syn::Error::new_spanned(
                arg,
                "use an explicit native receiver parameter, not self",
            )),
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let valid = match kind {
        Kind::Method => parameters.len() == 3 && named(parameters[1], "ArgValues") && vm_parameter(parameters[2]),
        Kind::ClassMethod => {
            parameters.len() == 5
                && named(parameters[0], "StaticStrings")
                && named(parameters[1], "Type")
                && named(parameters[2], "Value")
                && named(parameters[3], "ArgValues")
                && vm_parameter(parameters[4])
        }
        Kind::Value => parameters.len() == 1 && vm_parameter(parameters[0]),
    };
    let ReturnType::Type(_, result) = &signature.output else {
        return Err(syn::Error::new_spanned(
            signature,
            "Python attributes require an explicit return type",
        ));
    };
    let output_valid = match kind {
        Kind::Value => named(result, "Value"),
        Kind::Method | Kind::ClassMethod => {
            let output = if matches!(kind, Kind::Method) {
                "Value"
            } else {
                "CallResult"
            };
            type_path(result).is_ok_and(|segment| {
                segment.ident == "RunResult" && matches!(&segment.arguments, PathArguments::AngleBracketed(arguments)
                    if arguments.args.len() == 1 && matches!(arguments.args.first(), Some(GenericArgument::Type(ty)) if named(ty, output)))
            })
        }
    };
    if !valid || !output_valid {
        let expected = match kind {
            Kind::Method => "expected (native_receiver, ArgValues, &mut VM<'h>) -> RunResult<Value>",
            Kind::ClassMethod => {
                "expected (StaticStrings, Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>"
            }
            Kind::Value => "expected (&mut VM<'h>) -> Value",
        };
        return Err(syn::Error::new_spanned(signature, expected));
    }
    if matches!(kind, Kind::Method) {
        let first = parameters[0];
        let valid = match mode {
            Mode::Immediate => first == native,
            Mode::MutHeap | Mode::Heap => matches!(first, Type::Reference(reference)
            if (matches!(mode, Mode::MutHeap) || reference.mutability.is_none())
            && type_path(&reference.elem).is_ok_and(|segment| {
                (segment.ident == "HeapRead" || segment.ident == "HeapObjectRead")
                    && matches!(&segment.arguments, PathArguments::AngleBracketed(arguments)
                        if arguments.args.len() == 2
                            && matches!(arguments.args.first(), Some(GenericArgument::Lifetime(_)))
                            && matches!(arguments.args.last(), Some(GenericArgument::Type(ty)) if ty == native))
            })),
        };
        if !valid {
            return Err(syn::Error::new_spanned(
                first,
                "method receiver does not match Receiver; Heap<T> requires a shared handle, MutHeap<T> allows shared or mutable handles, Immediate<T> takes T by value",
            ));
        }
    }
    Ok(())
}

fn definition(owner: &Type, name: &syn::Ident, kind: Kind, mode: Mode, variant: &syn::Ident) -> TokenStream {
    let handler = quote!(<#owner>::#name);
    match kind {
        Kind::ClassMethod => quote!(crate::types::builtin_attr::AttrDef::ClassMethod(#handler)),
        Kind::Value => quote!(crate::types::builtin_attr::AttrDef::Value(#handler)),
        Kind::Method => {
            let body = match mode {
                Mode::Immediate => quote! {
                    let crate::value::Value::#variant(value) = receiver else {
                        unreachable!("builtin method receiver must match its attribute table")
                    };
                    #handler(*value, args, vm)
                },
                Mode::Heap | Mode::MutHeap => {
                    let (binding, argument) = if matches!(mode, Mode::MutHeap) {
                        (quote!(mut value), quote!(&mut value))
                    } else {
                        (quote!(value), quote!(&value))
                    };
                    quote! {
                        let crate::value::Value::Ref(id) = receiver else {
                            unreachable!("builtin method receiver must be a heap value")
                        };
                        let crate::heap::HeapReadOutput::#variant(#binding) = vm.heap.read(*id) else {
                            unreachable!("builtin method receiver must match its attribute table")
                        };
                        #handler(#argument, args, vm)
                    }
                }
            };
            quote! {
                crate::types::builtin_attr::AttrDef::Method({
                    fn call(receiver: &crate::value::Value, args: crate::args::ArgValues, vm: &mut crate::bytecode::VM<'_>) -> crate::exception_private::RunResult<crate::value::Value> {
                        #body
                    }
                    call
                })
            }
        }
    }
}

pub(crate) fn expand(mut input: ItemImpl) -> syn::Result<TokenStream> {
    let Some((negation, trait_path, _)) = &input.trait_ else {
        return Err(syn::Error::new_spanned(
            &input,
            "#[pyattrs] requires impl PyAttrs for a namespace type",
        ));
    };
    if negation.is_some()
        || trait_path
            .segments
            .last()
            .is_none_or(|segment| segment.ident != "PyAttrs" || !matches!(segment.arguments, PathArguments::None))
        || input.unsafety.is_some()
        || !input.generics.params.is_empty()
        || input.generics.where_clause.is_some()
    {
        return Err(syn::Error::new_spanned(
            &input,
            "#[pyattrs] requires a safe, concrete impl PyAttrs",
        ));
    }
    let receiver_items = input
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Type(item) if item.ident == "Receiver" => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    if receiver_items.len() != 1 {
        return Err(syn::Error::new_spanned(&input, "declare exactly one type Receiver"));
    }
    let receiver_item = receiver_items[0].clone();
    let (mode, native, variant) = receiver(&receiver_item.ty)?;
    let native = native.clone();
    let mut methods = Vec::new();
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    for item in &mut input.items {
        match item {
            ImplItem::Type(item) if item.ident == "Receiver" => {}
            ImplItem::Fn(method) => {
                let mut annotation = None;
                let mut kept = Vec::new();
                for attr in mem::take(&mut method.attrs) {
                    let kind = if attr.path().is_ident("pymethod") {
                        Some(Kind::Method)
                    } else if attr.path().is_ident("pyclassmethod") {
                        Some(Kind::ClassMethod)
                    } else if attr.path().is_ident("pyvalue") {
                        Some(Kind::Value)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        if annotation.is_some() {
                            return Err(syn::Error::new_spanned(
                                attr,
                                "use exactly one Python attribute annotation per function",
                            ));
                        }
                        if !matches!(attr.meta, syn::Meta::Path(_)) {
                            return Err(syn::Error::new_spanned(
                                attr,
                                "Python attribute annotations do not take arguments",
                            ));
                        }
                        annotation = Some(kind);
                    } else {
                        if attr
                            .path()
                            .segments
                            .last()
                            .is_some_and(|segment| segment.ident.to_string().starts_with("py"))
                        {
                            return Err(syn::Error::new_spanned(
                                attr,
                                "expected #[pymethod], #[pyclassmethod], or #[pyvalue]",
                            ));
                        }
                        kept.push(attr);
                    }
                }
                method.attrs = kept;
                if let Some(kind) = annotation {
                    if method
                        .attrs
                        .iter()
                        .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
                    {
                        return Err(syn::Error::new_spanned(
                            method,
                            "conditional Python attributes are not supported",
                        ));
                    }
                    validate(method, kind, mode, &native)?;
                    let name = &method.sig.ident;
                    let spelling = name.to_string().trim_start_matches("r#").to_owned();
                    let static_name = spelling
                        .split('_')
                        .map(|part| {
                            let mut chars = part.chars();
                            chars
                                .next()
                                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                                .unwrap_or_default()
                        })
                        .collect::<String>();
                    if static_name.is_empty() || !names.insert(static_name.clone()) {
                        return Err(syn::Error::new_spanned(
                            name,
                            "duplicate or empty Python attribute name",
                        ));
                    }
                    let mut static_name = syn::parse_str::<syn::Ident>(&static_name).map_err(|_| {
                        syn::Error::new_spanned(name, "method name must map to a StaticStrings variant")
                    })?;
                    static_name.set_span(name.span());
                    let def = definition(&input.self_ty, name, kind, mode, &variant);
                    entries.push((static_name, def));
                }
                methods.push(method.clone());
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    item,
                    "#[pyattrs] supports Receiver and associated functions only",
                ));
            }
        }
    }
    let owner = &input.self_ty;
    let attrs = &input.attrs;
    let trait_path = &input.trait_.as_ref().unwrap().1;
    let table = entries
        .iter()
        .map(|(name, def)| quote!((crate::intern::StaticStrings::#name, #def)));
    // Lookup and enumeration share the table's function pointers.
    let arms = entries.iter().enumerate().map(|(index, (name, _))| {
        quote!(crate::intern::StaticStrings::#name => Some(<Self as #trait_path>::ATTRS[#index].1))
    });
    Ok(quote! {
        #(#attrs)*
        impl #owner {
            #(#methods)*

            pub(crate) const fn lookup_attr(name: crate::intern::StaticStrings) -> Option<crate::types::builtin_attr::AttrDef> {
                match name {
                    #(#arms,)*
                    _ => None,
                }
            }
        }

        #(#attrs)*
        impl #trait_path for #owner {
            #receiver_item
            const ATTRS: &'static [(crate::intern::StaticStrings, crate::types::builtin_attr::AttrDef)] = &[
                #(#table,)*
            ];
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        process::{self, Command},
    };

    use syn::parse_quote;

    use super::*;

    fn error(input: ItemImpl) -> String {
        expand(input).unwrap_err().to_string()
    }

    #[test]
    fn preserves_native_method_and_generates_table_and_lookup() {
        let input: ItemImpl = parse_quote! {
            impl PyAttrs for ListImpl {
                type Receiver = MutHeap<List>;

                /// Sorting can call Python code.
                #[pymethod]
                pub(crate) fn sort<'h>(list: &mut HeapRead<'h, List>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
                    native_sort(list, args, vm)
                }
            }
        };
        let output = expand(input.clone()).unwrap();
        let file: syn::File = syn::parse2(output.clone()).unwrap();
        let syn::Item::Impl(inherent) = &file.items[0] else {
            panic!("expected inherent implementation")
        };
        let ImplItem::Fn(actual) = &inherent.items[0] else {
            panic!("expected sort")
        };
        let ImplItem::Fn(mut expected) = input.items[1].clone() else {
            panic!("expected sort")
        };
        expected.attrs.retain(|attr| !attr.path().is_ident("pymethod"));
        assert_eq!(actual, &expected);
        let output = output.to_string();
        assert!(output.contains("StaticStrings :: Sort"));
        assert!(output.contains("const ATTRS"));
        assert!(output.contains("const fn lookup_attr"));
        assert!(output.contains("HeapReadOutput :: List (mut value)"));
        assert!(output.contains("< ListImpl > :: sort"));
        assert!(!output.contains("pymethod"));
    }

    #[test]
    fn receiver_modes_and_attribute_kinds() {
        let shared = expand(parse_quote! {
            impl PyAttrs for TupleImpl {
                type Receiver = Heap<Tuple>;
                #[pymethod]
                fn count<'h>(tuple: &HeapRead<'h, Tuple>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> { todo!() }
            }
        }).unwrap().to_string();
        assert!(shared.contains("HeapReadOutput :: Tuple (value)"));
        assert!(!shared.contains("mut value"));

        let immediate = expand(parse_quote! {
            impl PyAttrs for FloatImpl {
                type Receiver = Immediate<f64>;
                #[pymethod]
                fn is_integer(value: f64, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> { todo!() }
                #[pyclassmethod]
                fn from_number(name: StaticStrings, owner: Type, receiver: Value, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> { todo!() }
                #[pyvalue]
                fn min(vm: &mut VM<'_>) -> Value { todo!() }
            }
        }).unwrap().to_string();
        assert!(immediate.contains("Value :: Float (value)"));
        assert!(immediate.contains("StaticStrings :: IsInteger"));
        assert!(immediate.contains("AttrDef :: ClassMethod (< FloatImpl > :: from_number)"));
        assert!(immediate.contains("AttrDef :: Value (< FloatImpl > :: min)"));
    }

    #[test]
    fn invalid_receiver_declarations() {
        assert!(error(parse_quote! { impl PyAttrs for X {} }).contains("exactly one type Receiver"));
        assert!(error(parse_quote! { impl PyAttrs for X { type Receiver = Box<List>; } }).contains("Receiver must be"));
        assert!(
            error(parse_quote! { impl PyAttrs for X { type Receiver = Heap; } }).contains("requires one native type")
        );
        assert!(
            error(parse_quote! { impl PyAttrs for X { type Receiver = Heap<List, Tuple>; } })
                .contains("requires one native type")
        );
        assert!(
            error(parse_quote! { impl PyAttrs for X { type Receiver = Immediate<String>; } })
                .contains("Immediate supports")
        );
    }

    #[test]
    fn invalid_signatures_and_annotations() {
        let cases = [
            (
                quote!(
                    #[pymethod]
                    fn count<'h>(
                        tuple: &mut HeapRead<'h, Tuple>,
                        args: ArgValues,
                        vm: &mut VM<'h>,
                    ) -> RunResult<Value> {
                        todo!()
                    }
                ),
                "receiver does not match",
            ),
            (
                quote!(
                    #[pymethod]
                    fn count(&self) -> RunResult<Value> {
                        todo!()
                    }
                ),
                "not self",
            ),
            (
                quote!(
                    #[pymethod]
                    fn count() -> RunResult<Value> {
                        todo!()
                    }
                ),
                "expected (native_receiver",
            ),
            (
                quote!(
                    #[pymethod]
                    async fn count() -> RunResult<Value> {
                        todo!()
                    }
                ),
                "safe synchronous",
            ),
            (
                quote!(
                    #[pymethod]
                    fn count<T>() -> RunResult<Value> {
                        todo!()
                    }
                ),
                "only lifetime parameters",
            ),
            (
                quote!(
                    #[pymethod]
                    fn count() {}
                ),
                "explicit return type",
            ),
            (
                quote!(
                    #[pymethod(name = Count)]
                    fn count() {}
                ),
                "do not take arguments",
            ),
            (
                quote!(
                    #[pymethod]
                    #[pyvalue]
                    fn count() {}
                ),
                "exactly one Python attribute",
            ),
            (
                quote!(
                    #[pyother]
                    fn count() {}
                ),
                "expected #[pymethod]",
            ),
            (
                quote!(
                    #[pymethod]
                    #[cfg(test)]
                    fn count() {}
                ),
                "conditional Python attributes",
            ),
        ];
        for (method, expected) in cases {
            let input = syn::parse2(quote! { impl PyAttrs for X { type Receiver = Heap<Tuple>; #method } }).unwrap();
            assert!(error(input).contains(expected), "expected diagnostic: {expected}");
        }
    }

    #[test]
    fn rust_rejects_invalid_receivers_and_duplicate_implementations() {
        let generated = expand(parse_quote! { impl PyAttrs for Namespace { type Receiver = Heap<Native>; } }).unwrap();
        let support = quote! {
            mod intern { pub enum StaticStrings { Missing } }
            mod types { pub mod builtin_attr { pub enum AttrDef {} } }
            trait PyAttrs { type Receiver; const ATTRS: &'static [(intern::StaticStrings, types::builtin_attr::AttrDef)]; }
            struct Heap<T>(std::marker::PhantomData<T>);
            struct Native;
            struct Namespace;
        };
        let directory = env::temp_dir().join(format!("monty-pyattrs-{}", process::id()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("check.rs");
        let compile = |tokens: TokenStream| {
            fs::write(&source, tokens.to_string()).unwrap();
            Command::new("rustc")
                .arg("--crate-type=lib")
                .arg("--emit=metadata")
                .arg("--edition=2024")
                .arg("--out-dir")
                .arg(&directory)
                .arg(&source)
                .output()
                .unwrap()
        };
        let valid = compile(quote!(#support #generated));
        assert!(valid.status.success(), "{}", String::from_utf8_lossy(&valid.stderr));
        let duplicate = compile(quote!(#support #generated #generated));
        assert!(!duplicate.status.success());
        assert!(String::from_utf8_lossy(&duplicate.stderr).contains("E0119"));
        let diagnostic = expand(parse_quote! {
            impl PyAttrs for Namespace { type Receiver = Box<Native>; }
        })
        .unwrap_err()
        .into_compile_error();
        let invalid = compile(quote!(#support #diagnostic));
        assert!(!invalid.status.success());
        assert!(
            String::from_utf8_lossy(&invalid.stderr).contains("Receiver must be MutHeap<T>, Heap<T>, or Immediate<T>")
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
