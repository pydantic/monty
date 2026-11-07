use std::{collections::HashSet, mem};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{GenericArgument, ImplItem, ImplItemFn, ItemImpl, PathArguments, Type, punctuated::Punctuated};

#[derive(Clone, Copy)]
enum Mode {
    HeapHandle,
    NativeView,
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

fn receiver(ty: &Type) -> syn::Result<(Mode, Type, syn::Ident)> {
    let view = type_path(ty)?;
    match view.ident.to_string().as_str() {
        "f64" | "bool" if matches!(view.arguments, PathArguments::None) => {
            let variant = match view.ident.to_string().as_str() {
                "f64" => "Float",
                _ => "Bool",
            };
            Ok((Mode::Immediate, ty.clone(), format_ident!("{variant}")))
        }
        "PyStr" | "PyBytes" | "PyInt" => {
            if !matches!(&view.arguments, PathArguments::AngleBracketed(args)
                if args.args.len() == 1 && matches!(args.args.first(), Some(GenericArgument::Lifetime(_))))
            {
                return Err(syn::Error::new_spanned(
                    ty,
                    format!("{} requires one heap lifetime", view.ident),
                ));
            }
            Ok((Mode::NativeView, ty.clone(), view.ident.clone()))
        }
        "HeapRead" | "HeapReadMut" | "HeapObjectRead" => {
            let PathArguments::AngleBracketed(args) = &view.arguments else {
                return Err(syn::Error::new_spanned(
                    ty,
                    "heap view requires a lifetime and native type",
                ));
            };
            if args.args.len() != 2 || !matches!(args.args.first(), Some(GenericArgument::Lifetime(_))) {
                return Err(syn::Error::new_spanned(
                    ty,
                    "heap view requires a lifetime and native type",
                ));
            }
            let Some(GenericArgument::Type(native)) = args.args.last() else {
                return Err(syn::Error::new_spanned(ty, "heap view requires a native type"));
            };
            let variant = type_path(native)?.ident.clone();
            Ok((Mode::HeapHandle, native.clone(), variant))
        }
        _ => Err(syn::Error::new_spanned(
            ty,
            "Receiver must be HeapRead<'h, T>, HeapReadMut<'h, T>, HeapObjectRead<'h, T>, PyStr<'h>, PyBytes<'h>, PyInt<'h>, f64, or bool",
        )),
    }
}

fn erase_path_lifetimes(path: &mut syn::Path) {
    for segment in &mut path.segments {
        if let PathArguments::AngleBracketed(args) = &mut segment.arguments {
            for arg in &mut args.args {
                if let GenericArgument::Lifetime(lifetime) = arg {
                    *lifetime = syn::Lifetime::new("'_", lifetime.apostrophe);
                }
            }
        }
    }
}

fn definition(
    owner: &Type,
    method: &ImplItemFn,
    kind: Kind,
    receiver: &(Mode, Type, syn::Ident),
    attr_name: &syn::Ident,
    defining_owner: Option<&syn::Ident>,
) -> TokenStream {
    let name = &method.sig.ident;
    let (mode, native, variant) = receiver;
    // Callable adapters must work for every VM lifetime, independently of the table's lifetime.
    let mut owner = owner.clone();
    if let Type::Path(owner) = &mut owner {
        erase_path_lifetimes(&mut owner.path);
    }
    let handler = quote!(<#owner>::#name);
    match kind {
        Kind::ClassMethod => quote! {
            crate::py_attrs::AttrDef::ClassMethod({
                fn call<'h>(this: crate::py_class::PyClass<'h>, args: crate::args::ArgValues, vm: &mut crate::bytecode::VM<'h>) -> crate::exception_private::RunResult<crate::bytecode::CallResult> {
                    #handler(crate::intern::StaticStrings::#attr_name, this, args, vm)
                }
                call
            })
        },
        Kind::Value => quote!(crate::py_attrs::AttrDef::Value(#handler)),
        Kind::Method => {
            let defining_owner =
                defining_owner.map_or_else(|| quote!(None), |owner| quote!(Some(crate::types::Type::#owner)));
            let body = match mode {
                Mode::Immediate => {
                    let pattern = quote!(crate::py_value::PyValue::#variant(value));
                    quote! {
                        let #pattern = this else {
                            return crate::py_attrs::invalid_receiver(crate::intern::StaticStrings::#attr_name, args, vm)
                        };
                        #handler(*value, args, vm)
                    }
                }
                Mode::NativeView => {
                    let variant = if variant == "PyBytes" {
                        format_ident!("Bytes")
                    } else if variant == "PyInt" {
                        format_ident!("Int")
                    } else {
                        format_ident!("Str")
                    };
                    quote! {
                        let crate::py_value::PyValue::#variant(value) = this else {
                            return crate::py_attrs::invalid_receiver(crate::intern::StaticStrings::#attr_name, args, vm)
                        };
                        #handler(value, args, vm)
                    }
                }
                Mode::HeapHandle => quote! {
                    let Some(value) = <#native as crate::py_value::PyValuePayload>::from_py_value(this) else {
                        return crate::py_attrs::invalid_receiver(crate::intern::StaticStrings::#attr_name, args, vm)
                    };
                    #handler(value, args, vm)
                },
            };
            quote! {
                crate::py_attrs::AttrDef::Method { owner: #defining_owner, call: {
                    fn call<'h>(this: &mut crate::py_value::PyValue<'h>, args: crate::args::ArgValues, vm: &mut crate::bytecode::VM<'h>) -> crate::exception_private::RunResult<crate::bytecode::CallResult> {
                        ({ #body }).map(Into::into)
                    }
                    call
                }}
            }
        }
    }
}

pub(crate) fn expand(mut input: ItemImpl) -> syn::Result<TokenStream> {
    if input.trait_.is_none() {
        return Err(syn::Error::new_spanned(
            &input,
            "#[pyattrs] requires a trait implementation",
        ));
    }
    if input.unsafety.is_some() || input.trait_.as_ref().is_some_and(|(negation, _, _)| negation.is_some()) {
        return Err(syn::Error::new_spanned(
            &input,
            "#[pyattrs] requires a safe positive trait implementation",
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
    let receiver = receiver(&receiver_item.ty)?;
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
                        let mut arguments = attr
                            .parse_args_with(Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated)?
                            .into_iter();
                        let name = arguments
                            .next()
                            .ok_or_else(|| syn::Error::new_spanned(&attr, "expected a StaticStrings key"))?;
                        let owner = arguments.next();
                        if arguments.next().is_some() || (owner.is_some() && !matches!(kind, Kind::Method)) {
                            return Err(syn::Error::new_spanned(
                                &attr,
                                "only methods accept an optional defining native type",
                            ));
                        }
                        annotation = Some((kind, name, owner));
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
                if let Some((kind, static_name, defining_owner)) = annotation {
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
                    if !names.insert(static_name.to_string()) {
                        return Err(syn::Error::new_spanned(static_name, "duplicate Python attribute name"));
                    }
                    let def = definition(
                        &input.self_ty,
                        method,
                        kind,
                        &receiver,
                        &static_name,
                        defining_owner.as_ref(),
                    );
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
    let generics = &input.generics;
    let inherent_generics = if matches!(type_path(owner)?.arguments, PathArguments::None) {
        syn::Generics::default()
    } else {
        input.generics.clone()
    };
    let mut lookup_trait = trait_path.clone();
    erase_path_lifetimes(&mut lookup_trait);
    let table = entries
        .iter()
        .map(|(name, def)| quote!((crate::intern::StaticStrings::#name, #def)));
    // Lookup and enumeration share the table's function pointers.
    let arms = entries.iter().enumerate().map(|(index, (name, _))| {
        quote!(crate::intern::StaticStrings::#name => Some(<Self as #lookup_trait>::TABLE.entries[#index].1))
    });
    Ok(quote! {
        #(#attrs)*
        impl #inherent_generics #owner {
            #(#methods)*

            pub(crate) const fn lookup_attr(name: crate::intern::StaticStrings) -> Option<crate::py_attrs::AttrDef> {
                match name {
                    #(#arms,)*
                    _ => None,
                }
            }
        }

        #(#attrs)*
        impl #generics #trait_path for #owner {
            #receiver_item
            const TABLE: crate::py_attrs::AttrTable = crate::py_attrs::AttrTable {
                entries: &[#(#table,)*],
                lookup: Self::lookup_attr,
            };
        }
    })
}
