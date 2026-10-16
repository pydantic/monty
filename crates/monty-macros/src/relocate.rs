//! Reconstructs stored graphs from integer references, never by deserializing pointers.

use proc_macro2::{Ident, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

/// Generates a conversion only on the integer-reference specialization of a stored type.
pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let mut parameters = input.generics.type_params();
    let reference = &parameters
        .next()
        .ok_or_else(|| syn::Error::new_spanned(input, "Relocate requires one reference parameter"))?
        .ident;
    if parameters.next().is_some() || input.generics.params.len() != 1 {
        return Err(syn::Error::new_spanned(
            input,
            "Relocate requires exactly one reference parameter",
        ));
    }
    let name = &input.ident;
    let body = match &input.data {
        Data::Struct(data) => {
            let (pattern, output) = fields(&data.fields, reference)?;
            quote! { let Self #pattern = self; #name #output }
        }
        Data::Enum(data) => {
            let arms = data
                .variants
                .iter()
                .map(|variant| {
                    let variant_name = &variant.ident;
                    let attrs: Vec<_> = variant
                        .attrs
                        .iter()
                        .filter(|attr| attr.path().is_ident("cfg"))
                        .collect();
                    let (pattern, output) = fields(&variant.fields, reference)?;
                    Ok(quote! { #(#attrs)* Self::#variant_name #pattern => #name::#variant_name #output, })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote!(match self { #(#arms)* })
        }
        Data::Union(_) => return Err(syn::Error::new_spanned(input, "unions cannot be relocated")),
    };
    Ok(quote! {
        impl #name<crate::heap::HeapIndex> {
            /// Rebuilds references using the destination owner's reserved slots.
            pub(crate) fn relocate<__To: crate::heap::RuntimeRef>(
                self,
                __reference: &mut impl FnMut(crate::heap::HeapIndex) -> __To,
            ) -> #name<__To> {
                #body
            }
        }
    })
}

/// Destructures fields once, preserving all reference-free data without cloning.
fn fields(fields: &Fields, reference: &Ident) -> syn::Result<(TokenStream, TokenStream)> {
    let bindings: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, _)| format_ident!("__field_{index}"))
        .collect();
    let mapped = fields
        .iter()
        .zip(&bindings)
        .map(|(field, binding)| map_type(&field.ty, quote!(#binding), reference))
        .collect::<syn::Result<Vec<_>>>()?;
    Ok(match fields {
        Fields::Named(fields) => {
            let names: Vec<_> = fields.named.iter().map(|field| &field.ident).collect();
            (quote!({ #(#names: #bindings,)* }), quote!({ #(#names: #mapped,)* }))
        }
        Fields::Unnamed(_) => (quote!((#(#bindings,)*)), quote!((#(#mapped,)*))),
        Fields::Unit => (quote!(), quote!()),
    })
}

/// Maps only types containing the reference parameter; native buffers move unchanged.
fn map_type(ty: &Type, value: TokenStream, reference: &Ident) -> syn::Result<TokenStream> {
    if !mentions(ty.to_token_stream(), reference) {
        return Ok(value);
    }
    match ty {
        Type::Path(path) => {
            let segment = path.path.segments.last().unwrap();
            if path.path.is_ident(reference) {
                return Ok(quote!(__reference(#value)));
            }
            let arguments: Vec<_> = match &segment.arguments {
                PathArguments::AngleBracketed(args) => args
                    .args
                    .iter()
                    .filter_map(|arg| match arg {
                        GenericArgument::Type(ty) => Some(ty),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let name = segment.ident.to_string();
            match name.as_str() {
                "PendingChildren" => {
                    Ok(quote!(#value.into_iter().map(|(__key, __item)| (__reference(__key), __item)).collect()))
                }
                "Box" if matches!(arguments[0], Type::Slice(_)) => {
                    let Type::Slice(slice) = arguments[0] else {
                        unreachable!()
                    };
                    let mapped = map_type(&slice.elem, quote!(__item), reference)?;
                    Ok(
                        quote!(#value.into_vec().into_iter().map(|__item| #mapped).collect::<Vec<_>>().into_boxed_slice()),
                    )
                }
                "Option" | "Vec" | "VecDeque" | "HashSet" | "AHashSet" | "BTreeSet" | "SmallVec" | "TupleVec"
                | "BoxOn32" | "Box" | "Cell" | "UnsafeCell" | "PyHashTable" => {
                    let element = if name == "TupleVec" {
                        syn::parse_quote!(crate::value::Value<#reference>)
                    } else if name == "SmallVec" {
                        let Type::Array(array) = arguments[0] else {
                            return Err(syn::Error::new_spanned(ty, "expected SmallVec array"));
                        };
                        (*array.elem).clone()
                    } else {
                        arguments[0].clone()
                    };
                    let mapped = map_type(&element, quote!(__item), reference)?;
                    Ok(match name.as_str() {
                        "Option" => quote!(#value.map(|__item| #mapped)),
                        "Box" => quote!(Box::new({ let __item = *#value; #mapped })),
                        "BoxOn32" => quote!(crate::heap::relocate::map_box_on_32(#value, |__item| #mapped)),
                        "Cell" => quote!(::std::cell::Cell::new({ let __item = #value.into_inner(); #mapped })),
                        "UnsafeCell" => {
                            quote!(::std::cell::UnsafeCell::new({ let __item = #value.into_inner(); #mapped }))
                        }
                        "PyHashTable" => quote!(#value.map_entries(|__item| #mapped)),
                        _ => quote!(#value.into_iter().map(|__item| #mapped).collect()),
                    })
                }
                "HashMap" | "AHashMap" | "BTreeMap" => {
                    let key = map_type(arguments[0], quote!(__key), reference)?;
                    let item = map_type(arguments[1], quote!(__item), reference)?;
                    Ok(quote!(#value.into_iter().map(|(__key, __item)| (#key, #item)).collect()))
                }
                _ => Ok(quote!(#value.relocate(__reference))),
            }
        }
        Type::Group(group) => map_type(&group.elem, value, reference),
        Type::Paren(group) => map_type(&group.elem, value, reference),
        Type::Tuple(tuple) => {
            let bindings: Vec<_> = tuple
                .elems
                .iter()
                .enumerate()
                .map(|(index, _)| format_ident!("__part_{index}"))
                .collect();
            let mapped = tuple
                .elems
                .iter()
                .zip(&bindings)
                .map(|(ty, name)| map_type(ty, quote!(#name), reference))
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote!({ let (#(#bindings,)*) = #value; (#(#mapped,)*) }))
        }
        Type::Array(array) => {
            let mapped = map_type(&array.elem, quote!(__item), reference)?;
            Ok(quote!(#value.map(|__item| #mapped)))
        }
        _ => Err(syn::Error::new_spanned(ty, "unsupported reference-bearing storage")),
    }
}

/// Finds the parameter in nested type syntax without interpreting unrelated paths.
fn mentions(tokens: TokenStream, reference: &Ident) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => ident == *reference,
        TokenTree::Group(group) => mentions(group.stream(), reference),
        _ => false,
    })
}
