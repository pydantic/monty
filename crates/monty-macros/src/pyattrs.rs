use std::{collections::HashSet, mem};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Attribute, GenericArgument, ImplItem, ItemImpl, PathArguments, Type, punctuated::Punctuated};

pub(crate) fn expand(mut input: ItemImpl) -> syn::Result<TokenStream> {
    if input.trait_.is_none() {
        return Err(syn::Error::new_spanned(
            &input,
            "#[pyattrs] requires a trait implementation",
        ));
    }
    let receiver = input.items.iter().find_map(|item| match item {
        ImplItem::Type(item) if item.ident == "Receiver" => Some(&item.ty),
        _ => None,
    });
    let projection =
        project_receiver(receiver.ok_or_else(|| syn::Error::new_spanned(&input, "declare type Receiver"))?)?;
    // Both impls are generated, even though their bodies retain the source's spans.
    input.impl_token = syn::Token![impl](Span::call_site());
    let mut table_impl = input.clone();
    table_impl.items.retain(|item| matches!(item, ImplItem::Type(_)));
    input.trait_ = None;
    if let Type::Path(owner) = &*input.self_ty
        && owner.path.segments.iter().all(|segment| segment.arguments.is_empty())
    {
        input.generics = syn::Generics::default();
    }
    let owner = inferred_lifetimes((*input.self_ty).clone());
    let mut names = HashSet::new();
    let mut entries = Vec::new();
    let mut arms = Vec::new();

    for item in &mut input.items {
        match item {
            ImplItem::Type(item) if item.ident == "Receiver" => {}
            ImplItem::Fn(method) => {
                let mut annotation = None;
                let mut remaining = Vec::new();
                for attr in mem::take(&mut method.attrs) {
                    if let Some(kind) = Kind::from_attr(&attr) {
                        if annotation.is_some() {
                            return Err(syn::Error::new_spanned(
                                attr,
                                "use one Python attribute annotation per function",
                            ));
                        }
                        let args = attr.parse_args_with(Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated)?;
                        let mut args = args.into_iter();
                        let name = args
                            .next()
                            .ok_or_else(|| syn::Error::new_spanned(&attr, "expected a StaticStrings key"))?;
                        let owner = args.next();
                        if args.next().is_some() || (owner.is_some() && !matches!(kind, Kind::Method)) {
                            return Err(syn::Error::new_spanned(
                                attr,
                                "only methods accept an optional defining native type",
                            ));
                        }
                        annotation = Some((kind, name, owner));
                    } else {
                        remaining.push(attr);
                    }
                }
                method.attrs = remaining;
                let Some((kind, name, defining_owner)) = annotation else {
                    continue;
                };
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
                if !names.insert(name.to_string()) {
                    return Err(syn::Error::new_spanned(name, "duplicate Python attribute name"));
                }
                let handler = &method.sig.ident;
                let key = quote!(crate::intern::StaticStrings::#name);
                let definition = match kind {
                    Kind::Method => {
                        let defining_owner = defining_owner
                            .map_or_else(|| quote!(None), |owner| quote!(Some(crate::types::Type::#owner)));
                        quote! {
                            crate::py_attrs::AttrDef::Method { owner: #defining_owner, call: |this, args, vm| {
                                let Some(receiver) = (#projection) else {
                                    return crate::py_attrs::invalid_receiver(#key, args, vm);
                                };
                                <#owner>::#handler(receiver, args, vm).map(Into::into)
                            }}
                        }
                    }
                    Kind::ClassMethod => quote! {
                        crate::py_attrs::AttrDef::ClassMethod(|this, args, vm| <#owner>::#handler(#key, this, args, vm))
                    },
                    Kind::Value => quote!(crate::py_attrs::AttrDef::Value(<#owner>::#handler)),
                };
                let index = entries.len();
                entries.push(quote!((#key, #definition)));
                arms.push(quote!(#key => Some(Self::TABLE.entries[#index].1)));
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    item,
                    "#[pyattrs] supports Receiver and associated functions only",
                ));
            }
        }
    }
    input.items.retain(|item| matches!(item, ImplItem::Fn(_)));
    table_impl.items.push(syn::parse_quote! {
        const TABLE: crate::py_attrs::AttrTable = crate::py_attrs::AttrTable {
            entries: &[#(#entries,)*],
            lookup: |name| match name {
                #(#arms,)*
                _ => None,
            },
        };
    });
    Ok(quote!(#input #table_impl))
}

#[derive(Clone, Copy)]
enum Kind {
    Method,
    ClassMethod,
    Value,
}

impl Kind {
    fn from_attr(attr: &Attribute) -> Option<Self> {
        if attr.path().is_ident("pymethod") {
            Some(Self::Method)
        } else if attr.path().is_ident("pyclassmethod") {
            Some(Self::ClassMethod)
        } else if attr.path().is_ident("pyvalue") {
            Some(Self::Value)
        } else {
            None
        }
    }
}

/// Projects a native view; rustc checks the receiver's generic arguments and handler signature.
fn project_receiver(ty: &Type) -> syn::Result<TokenStream> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(ty, "expected a named receiver type"));
    };
    let segment = path.path.segments.last().expect("type paths are nonempty");
    let (variant, value) = match segment.ident.to_string().as_str() {
        "PyStr" => (quote!(Str), quote!(value)),
        "PyBytes" => (quote!(Bytes), quote!(value)),
        "PyInt" => (quote!(Int), quote!(value)),
        "f64" => (quote!(Float), quote!(*value)),
        "bool" => (quote!(Bool), quote!(*value)),
        "HeapRead" | "HeapReadMut" | "HeapObjectRead" => {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                return Err(syn::Error::new_spanned(ty, "expected a heap payload type"));
            };
            let native = args
                .args
                .iter()
                .find_map(|arg| match arg {
                    GenericArgument::Type(ty) => Some(ty),
                    _ => None,
                })
                .ok_or_else(|| syn::Error::new_spanned(ty, "expected a heap payload type"))?;
            return Ok(quote!(<#native as crate::py_value::PyValuePayload>::from_py_value(this)));
        }
        _ => return Err(syn::Error::new_spanned(ty, "unsupported native receiver type")),
    };
    Ok(quote! {
        match this {
            crate::py_value::PyValue::#variant(value) => Some(#value),
            _ => None,
        }
    })
}

/// Adapters must accept every VM lifetime, independently of the namespace impl's lifetime.
fn inferred_lifetimes(mut owner: Type) -> Type {
    if let Type::Path(path) = &mut owner {
        for segment in &mut path.path.segments {
            if let PathArguments::AngleBracketed(args) = &mut segment.arguments {
                for arg in &mut args.args {
                    if let GenericArgument::Lifetime(lifetime) = arg {
                        *lifetime = syn::Lifetime::new("'_", lifetime.apostrophe);
                    }
                }
            }
        }
    }
    owner
}
