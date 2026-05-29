//! Codegen for `#[derive(ToArgs)]`. Inverse of `#[derive(FromArgs)]`:
//! projects a struct into the `(Vec<MontyObject>, kwargs)` pair host
//! callbacks expect. Reuses `from_args` field classification so structs
//! that derive both stay symmetric.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Fields, Ident, LitStr, Type, spanned::Spanned};

use crate::from_args::{FieldKind, parse_field_attrs};

/// Field metadata for codegen. `kind` mirrors `FromArgs` *after* the
/// implicit kw_only-after-varargs rule.
struct ProjField {
    ident: Ident,
    ty: Type,
    kind: FieldKind,
}

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let DeriveInput {
        ident: struct_ident,
        data,
        ..
    } = input;

    // Unit structs yield an empty vec → trivial impl.
    let named_fields: Vec<&syn::Field> = match data {
        Data::Struct(DataStruct {
            fields: Fields::Named(named),
            ..
        }) => named.named.iter().collect(),
        Data::Struct(DataStruct {
            fields: Fields::Unit, ..
        }) => Vec::new(),
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "ToArgs can only be derived for structs with named fields or unit structs",
            ));
        }
    };

    let mut proj_fields: Vec<ProjField> = Vec::with_capacity(named_fields.len());
    let mut seen_varargs = false;
    for field in named_fields {
        let attrs = parse_field_attrs(&field.attrs)?;
        let ident = field.ident.clone().expect("named field");
        // Mirror FromArgs' "implicit kw_only after varargs" rule.
        let mut kind = attrs.kind;
        match kind {
            FieldKind::Varargs => seen_varargs = true,
            FieldKind::PosOrKeyword if seen_varargs => kind = FieldKind::KwOnly,
            _ => {}
        }
        proj_fields.push(ProjField {
            ident,
            ty: field.ty.clone(),
            kind,
        });
    }

    let pos_pushes = proj_fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;
        match f.kind {
            FieldKind::PosOnly | FieldKind::PosOrKeyword => quote! {
                __pos.push(
                    <#ty as crate::args::ToMontyObject>::into_monty_object(self.#ident),
                );
            },
            FieldKind::Varargs => quote! {
                for __item in self.#ident {
                    __pos.push(
                        <_ as crate::args::ToMontyObject>::into_monty_object(__item),
                    );
                }
            },
            _ => quote! {},
        }
    });

    let kw_pushes = proj_fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;
        let name_lit = LitStr::new(&ident.to_string(), ident.span());
        match f.kind {
            FieldKind::KwOnly => quote! {
                __kw.push((
                    crate::MontyObject::String(#name_lit.to_owned()),
                    <#ty as crate::args::ToMontyObject>::into_monty_object(self.#ident),
                ));
            },
            FieldKind::Varkwargs => {
                // No caller needs this yet; reject at codegen rather than
                // silently dropping the field.
                let span = ident.span();
                let err = syn::Error::new(span, "ToArgs does not yet support `varkwargs` fields").to_compile_error();
                quote! { #err }
            }
            _ => quote! {},
        }
    });

    Ok(quote! {
        #[automatically_derived]
        impl crate::args::ToArgs for #struct_ident {
            fn to_args(
                self,
            ) -> (
                ::std::vec::Vec<crate::MontyObject>,
                ::std::vec::Vec<(crate::MontyObject, crate::MontyObject)>,
            ) {
                let mut __pos: ::std::vec::Vec<crate::MontyObject> = ::std::vec::Vec::new();
                let mut __kw: ::std::vec::Vec<(crate::MontyObject, crate::MontyObject)> =
                    ::std::vec::Vec::new();
                #(#pos_pushes)*
                #(#kw_pushes)*
                (__pos, __kw)
            }
        }
    })
}
