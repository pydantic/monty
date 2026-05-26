//! Codegen for the `#[derive(FromArgs)]` macro.
//!
//! Parses a struct definition with `#[from_args(...)]` attributes into a
//! validated `Signature`, then renders the body of a `from_args` method that
//! drives positional and keyword argument dispatch off of an `ArgValues`.
//!
//! The output is hard-coded against monty-internal paths (`crate::args::...`,
//! `crate::exception_private::ExcType`, etc.) because this derive is only used
//! from inside the `monty` crate itself. Cross-crate usage would require
//! switching to `::monty::...` paths plus a `proc-macro-crate` lookup.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Data, DataStruct, DeriveInput, Expr, Fields, Ident, LitStr, Token, Type, spanned::Spanned};

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let signature = Signature::parse(input)?;
    Ok(signature.render())
}

/// Parsed, validated signature for a single struct deriving `FromArgs`.
struct Signature {
    /// The struct's identifier (used as `Self`).
    struct_ident: Ident,
    /// Function name used in error messages (e.g. positional-keyword conflict).
    func_name: String,
    /// Fields in declaration order — also the order of positional arguments.
    fields: Vec<Field>,
    /// Index of the `*args` field (if any).
    varargs_idx: Option<usize>,
    /// Index of the `**kwargs` field (if any).
    varkwargs_idx: Option<usize>,
    /// Which `type_error_c_at_most*` helper to emit when too many positional
    /// arguments are passed. Matches CPython's per-constructor wording.
    at_most_style: AtMostStyle,
}

/// Selects the wording of the "too many positional args" `TypeError` message.
#[derive(Clone, Copy)]
enum AtMostStyle {
    /// `function takes at most {max} arguments ({actual} given)` — default,
    /// matches most C-implemented constructors (e.g. `date`).
    Standard,
    /// `function takes at most {max} positional arguments ({actual} given)`
    /// — used by constructors that want to disambiguate from kwargs
    /// (e.g. `datetime`).
    Positional,
}

/// A single field of a `FromArgs` struct, with its kind and per-field options.
struct Field {
    ident: Ident,
    ty: Type,
    kind: FieldKind,
    /// Default if absent (`None` = required).
    default: Option<DefaultExpr>,
    /// Explicit `StaticStrings::Variant` override for kwarg dispatch.
    static_string: Option<Ident>,
    /// 1-indexed position in the *positional-or-keyword* region (for error
    /// messages that mention "pos N"). `None` for `kw_only`/`varargs`/
    /// `varkwargs` fields.
    pos_index: Option<usize>,
}

/// Role of a field in the signature.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum FieldKind {
    /// Accepts either positional or keyword.
    #[default]
    PosOrKeyword,
    /// Accepts positional only.
    PosOnly,
    /// Accepts keyword only.
    KwOnly,
    /// `*args` — collects remaining positionals.
    Varargs,
    /// `**kwargs` — collects unmatched kwargs.
    Varkwargs,
}

/// Source of a field's default value.
enum DefaultExpr {
    /// `#[from_args(default)]` — call `Default::default()`.
    DefaultTrait,
    /// `#[from_args(default = <expr>)]` — evaluate `<expr>`.
    Explicit(Box<Expr>),
}

impl Signature {
    fn parse(input: &DeriveInput) -> syn::Result<Self> {
        let DeriveInput {
            ident: struct_ident,
            data,
            attrs,
            ..
        } = input;

        let Data::Struct(DataStruct {
            fields: Fields::Named(named),
            ..
        }) = data
        else {
            return Err(syn::Error::new(
                input.span(),
                "FromArgs can only be derived for structs with named fields",
            ));
        };

        let (func_name, at_most_style) = parse_struct_attrs(attrs)?;

        let mut fields = Vec::with_capacity(named.named.len());
        let mut varargs_idx = None;
        let mut varkwargs_idx = None;

        for field in &named.named {
            let opts = parse_field_attrs(&field.attrs)?;
            let ident = field.ident.clone().expect("named field");
            // Defer assigning `kind` to the next pass — we need full context
            // (e.g. "has there been a varargs?") to fill in implicit kw_only.
            fields.push(Field {
                ident,
                ty: field.ty.clone(),
                kind: opts.kind,
                default: opts.default,
                static_string: opts.static_string,
                pos_index: None,
            });
        }

        // Second pass: resolve implicit kw_only after varargs, validate ordering,
        // assign 1-based positional indices, and locate the varargs/varkwargs slots.
        let mut seen_varargs = false;
        let mut seen_varkwargs = false;
        let mut seen_pos_or_kw = false;
        let mut seen_kw_only = false;
        let mut pos_counter: usize = 0;
        for (idx, field) in fields.iter_mut().enumerate() {
            if seen_varkwargs {
                return Err(syn::Error::new(
                    field.ident.span(),
                    "no fields may appear after a `#[from_args(varkwargs)]` field",
                ));
            }

            match field.kind {
                FieldKind::PosOnly => {
                    if seen_pos_or_kw || seen_kw_only || seen_varargs {
                        return Err(syn::Error::new(
                            field.ident.span(),
                            "positional-only fields must come before positional-or-keyword, varargs, and keyword-only fields",
                        ));
                    }
                }
                FieldKind::PosOrKeyword => {
                    if seen_varargs {
                        // Implicit kw_only after varargs.
                        field.kind = FieldKind::KwOnly;
                        seen_kw_only = true;
                    } else if seen_kw_only {
                        return Err(syn::Error::new(
                            field.ident.span(),
                            "positional-or-keyword fields cannot appear after keyword-only fields",
                        ));
                    } else {
                        seen_pos_or_kw = true;
                    }
                }
                FieldKind::KwOnly => {
                    seen_kw_only = true;
                }
                FieldKind::Varargs => {
                    if seen_varargs {
                        return Err(syn::Error::new(
                            field.ident.span(),
                            "only one `#[from_args(varargs)]` field is allowed",
                        ));
                    }
                    seen_varargs = true;
                    varargs_idx = Some(idx);
                }
                FieldKind::Varkwargs => {
                    if seen_varkwargs {
                        return Err(syn::Error::new(
                            field.ident.span(),
                            "only one `#[from_args(varkwargs)]` field is allowed",
                        ));
                    }
                    seen_varkwargs = true;
                    varkwargs_idx = Some(idx);
                }
            }

            if matches!(field.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) {
                pos_counter += 1;
                field.pos_index = Some(pos_counter);
            }
        }

        Ok(Self {
            struct_ident: struct_ident.clone(),
            func_name,
            fields,
            varargs_idx,
            varkwargs_idx,
            at_most_style,
        })
    }

    fn render(&self) -> TokenStream {
        let struct_ident = &self.struct_ident;

        // Per-field temporary slot identifiers.
        let slots: Vec<Ident> = self
            .fields
            .iter()
            .map(|f| format_ident!("__slot_{}", f.ident))
            .collect();

        // Maximum number of named positional slots (for `at most N` errors).
        let max_positional = self.named_positional_count();
        let has_varargs = self.varargs_idx.is_some();
        let has_varkwargs = self.varkwargs_idx.is_some();

        let slot_decls = self.render_slot_decls(&slots);
        let cleanup_block = self.render_cleanup_block(&slots);
        let positional_loop = self.render_positional_loop(&slots, max_positional, has_varargs);
        let kwarg_loop = self.render_kwarg_loop(&slots, has_varkwargs);
        let build_struct = self.render_build_struct(&slots);

        quote! {
            #[automatically_derived]
            impl #struct_ident {
                /// Extracts the arguments into `Self`, returning a `TypeError` on
                /// argument-count, type, or duplicate-kwarg violations.
                ///
                /// On any error path, all already-extracted heap-owning fields are
                /// dropped via `DropWithHeap` so reference counts stay correct.
                pub(crate) fn from_args(
                    args: crate::args::ArgValues,
                    heap: &mut crate::heap::Heap<impl crate::resource::ResourceTracker>,
                    interns: &crate::intern::Interns,
                ) -> crate::exception_private::RunResult<Self> {
                    use crate::args::FromValue as _; // allow local import
                    use crate::heap::DropWithHeap as _; // allow local import

                    let (mut __pos_iter, __kwargs_holder) = args.into_parts();
                    let mut __kwargs_iter = __kwargs_holder.into_iter();

                    #slot_decls

                    // Macro `__cleanup!` drops every owning slot and returns the
                    // given error. Inlined here so it captures all field slots
                    // by name without needing to thread them through helpers.
                    macro_rules! __cleanup {
                        ($err:expr) => {{
                            #cleanup_block
                            // Also drop anything left in the iterators.
                            __pos_iter.drop_with_heap(heap);
                            __kwargs_iter.drop_with_heap(heap);
                            return Err($err);
                        }};
                    }

                    #positional_loop
                    #kwarg_loop

                    #build_struct
                }
            }
        }
    }

    fn named_positional_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword))
            .count()
    }

    fn render_slot_decls(&self, slots: &[Ident]) -> TokenStream {
        let decls = self.fields.iter().zip(slots).map(|(field, slot)| {
            let ty = &field.ty;
            match field.kind {
                FieldKind::Varargs => {
                    // Varargs accumulator: a `Vec<T>` of the element type.
                    let elem = vec_element_ty(ty).unwrap_or_else(|| ty.clone());
                    quote! {
                        let mut #slot: ::std::vec::Vec<#elem> = ::std::vec::Vec::new();
                    }
                }
                FieldKind::Varkwargs => {
                    // Varkwargs accumulator: a `Vec<(StringId, Value)>` we wrap as
                    // `KwargsValues::Inline` at the end.
                    quote! {
                        let mut #slot: ::std::vec::Vec<(
                            crate::intern::StringId,
                            crate::value::Value,
                        )> = ::std::vec::Vec::new();
                    }
                }
                _ => {
                    // Named positional / kw_only: Option<T> distinguishes "absent"
                    // from "present", driving both default fallback and
                    // duplicate-detection on the kwarg dispatch path.
                    quote! {
                        let mut #slot: ::std::option::Option<#ty> = ::std::option::Option::None;
                    }
                }
            }
        });
        quote! { #(#decls)* }
    }

    fn render_cleanup_block(&self, slots: &[Ident]) -> TokenStream {
        // Drop every owning slot. Non-owning (primitive) `Option<T>` slots are
        // dropped normally and that's a no-op for refcounts, so we don't need
        // to discriminate — but generating per-field drops in the macro body
        // would be wasteful, so we only emit drops for kinds that hold values.
        let drops = self.fields.iter().zip(slots).map(|(field, slot)| match field.kind {
            FieldKind::Varargs => {
                quote! {
                    let __taken = ::std::mem::take(&mut #slot);
                    __taken.drop_with_heap(heap);
                }
            }
            FieldKind::Varkwargs => {
                quote! {
                    for (_, __v) in ::std::mem::take(&mut #slot) {
                        __v.drop_with_heap(heap);
                    }
                }
            }
            _ => {
                let ty = &field.ty;
                quote! {
                    if let ::std::option::Option::Some(__v) = #slot.take() {
                        <#ty as crate::args::FromValue>::drop_extracted(__v, heap);
                    }
                }
            }
        });
        quote! { #(#drops)* }
    }

    fn render_positional_loop(&self, slots: &[Ident], max_positional: usize, has_varargs: bool) -> TokenStream {
        // Build the per-index arms by iterating fields that can accept positionals.
        let mut arms: Vec<TokenStream> = Vec::new();
        let mut arm_idx: usize = 0;
        for (field, slot) in self.fields.iter().zip(slots) {
            if !matches!(field.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) {
                continue;
            }
            let ty = &field.ty;
            let arm_idx_lit = arm_idx;
            arms.push(quote! {
                #arm_idx_lit => {
                    match <#ty as crate::args::FromValue>::from_value(__arg, heap, interns) {
                        ::std::result::Result::Ok(__v) => {
                            #slot = ::std::option::Option::Some(__v);
                        }
                        ::std::result::Result::Err(__e) => {
                            __cleanup!(__e);
                        }
                    }
                }
            });
            arm_idx += 1;
        }

        // Tail: either dispatch into varargs, or raise "at most N".
        let tail = if let Some(varargs_idx) = self.varargs_idx {
            let varargs_slot = &slots[varargs_idx];
            let elem_ty =
                vec_element_ty(&self.fields[varargs_idx].ty).unwrap_or_else(|| self.fields[varargs_idx].ty.clone());
            quote! {
                _ => {
                    match <#elem_ty as crate::args::FromValue>::from_value(__arg, heap, interns) {
                        ::std::result::Result::Ok(__v) => {
                            #varargs_slot.push(__v);
                        }
                        ::std::result::Result::Err(__e) => {
                            __cleanup!(__e);
                        }
                    }
                }
            }
        } else {
            let max_lit = max_positional;
            let helper = match self.at_most_style {
                AtMostStyle::Standard => quote!(type_error_c_at_most),
                AtMostStyle::Positional => quote!(type_error_c_at_most_positional),
            };
            quote! {
                _ => {
                    // The argument itself has not yet been consumed by from_value,
                    // so drop it explicitly before bubbling the count error.
                    __arg.drop_with_heap(heap);
                    let __actual = __pos_count + 1;
                    __cleanup!(crate::exception_private::ExcType::#helper(#max_lit, __actual));
                }
            }
        };
        let _ = has_varargs;

        quote! {
            let mut __pos_count: usize = 0;
            while let ::std::option::Option::Some(__arg) = ::std::iter::Iterator::next(&mut __pos_iter) {
                match __pos_count {
                    #(#arms)*
                    #tail
                }
                __pos_count += 1;
            }
        }
    }

    fn render_kwarg_loop(&self, slots: &[Ident], has_varkwargs: bool) -> TokenStream {
        // Build kwarg dispatch arms — only for fields that can be passed by name.
        let mut arms: Vec<TokenStream> = Vec::new();
        for (field, slot) in self.fields.iter().zip(slots) {
            let arm = match field.kind {
                FieldKind::PosOnly | FieldKind::Varargs | FieldKind::Varkwargs => continue,
                FieldKind::PosOrKeyword => kwarg_arm_pos_or_kw(field, slot, &self.func_name),
                FieldKind::KwOnly => kwarg_arm_kw_only(field, slot, &self.func_name),
            };
            arms.push(arm);
        }

        let unknown_arm = if let Some(varkwargs_idx) = self.varkwargs_idx {
            let varkwargs_slot = &slots[varkwargs_idx];
            quote! {
                _ => {
                    // Preserve the key — we need to retain its string id alongside
                    // the value in the varkwargs accumulator.
                    let Some(__id) = __key_str.string_id() else {
                        // Heap string key — intern it so we can carry a StringId.
                        // For now, reject heap-string keys passed to **kwargs.
                        // (TODO: support by allocating a StringId via Interns.)
                        __value.drop_with_heap(heap);
                        __key.drop_with_heap(heap);
                        __cleanup!(crate::exception_private::ExcType::type_error_kwargs_nonstring_key());
                    };
                    __key.drop_with_heap(heap);
                    #varkwargs_slot.push((__id, __value));
                }
            }
        } else {
            quote! {
                _ => {
                    __value.drop_with_heap(heap);
                    let __unexpected = __key_str.as_str(interns).to_owned();
                    __key.drop_with_heap(heap);
                    __cleanup!(crate::exception_private::ExcType::type_error_c_unexpected_keyword(&__unexpected));
                }
            }
        };
        let _ = has_varkwargs;

        // Build pos-only kwarg rejection arms (CPython error message style).
        let mut pos_only_arms: Vec<TokenStream> = Vec::new();
        for field in &self.fields {
            if matches!(field.kind, FieldKind::PosOnly) {
                let static_string_ident = field.static_string_variant();
                let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
                let func_name = &self.func_name;
                pos_only_arms.push(quote! {
                    ::std::option::Option::Some(__id) if __id == crate::intern::StaticStrings::#static_string_ident => {
                        __value.drop_with_heap(heap);
                        __key.drop_with_heap(heap);
                        __cleanup!(crate::exception_private::ExcType::type_error_positional_only(#func_name, #field_name_lit));
                    }
                });
            }
        }

        quote! {
            while let ::std::option::Option::Some((__key, __value)) = ::std::iter::Iterator::next(&mut __kwargs_iter) {
                let ::std::option::Option::Some(__key_str) = __key.as_either_str(heap) else {
                    __value.drop_with_heap(heap);
                    __key.drop_with_heap(heap);
                    __cleanup!(crate::exception_private::ExcType::type_error_kwargs_nonstring_key());
                };
                match __key_str.string_id() {
                    #(#pos_only_arms)*
                    #(#arms)*
                    #unknown_arm
                }
            }
        }
    }

    fn render_build_struct(&self, slots: &[Ident]) -> TokenStream {
        let func_name = self.func_name.as_str();
        let fields = self.fields.iter().zip(slots).map(|(field, slot)| {
            let ident = &field.ident;
            match field.kind {
                FieldKind::Varargs | FieldKind::Varkwargs => {
                    if matches!(field.kind, FieldKind::Varkwargs) {
                        // Wrap accumulated pairs as KwargsValues. An empty Vec
                        // collapses to `Empty` so callers can cheap-check.
                        quote! {
                            #ident: if #slot.is_empty() {
                                crate::args::KwargsValues::Empty
                            } else {
                                crate::args::KwargsValues::Inline(#slot)
                            },
                        }
                    } else {
                        quote! {
                            #ident: #slot,
                        }
                    }
                }
                _ => match &field.default {
                    None => {
                        let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
                        let pos = field.pos_index.unwrap_or(0);
                        if field.pos_index.is_some() {
                            quote! {
                                #ident: match #slot {
                                    ::std::option::Option::Some(__v) => __v,
                                    ::std::option::Option::None => {
                                        __cleanup!(crate::exception_private::ExcType::type_error_c_missing_required(#field_name_lit, #pos));
                                    }
                                },
                            }
                        } else {
                            // Required keyword-only argument.
                            quote! {
                                #ident: match #slot {
                                    ::std::option::Option::Some(__v) => __v,
                                    ::std::option::Option::None => {
                                        __cleanup!(crate::exception_private::ExcType::type_error_missing_kwonly_with_names(
                                            #func_name,
                                            &[#field_name_lit],
                                        ));
                                    }
                                },
                            }
                        }
                    }
                    Some(DefaultExpr::DefaultTrait) => quote! {
                        #ident: #slot.unwrap_or_default(),
                    },
                    Some(DefaultExpr::Explicit(expr)) => quote! {
                        #ident: #slot.unwrap_or_else(|| { #expr }),
                    },
                },
            }
        });
        quote! {
            ::std::result::Result::Ok(Self {
                #(#fields)*
            })
        }
    }
}

fn kwarg_arm_pos_or_kw(field: &Field, slot: &Ident, func_name: &str) -> TokenStream {
    let static_string_ident = field.static_string_variant();
    let ty = &field.ty;
    let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
    let pos = field.pos_index.unwrap_or(0);
    quote! {
        ::std::option::Option::Some(__id) if __id == crate::intern::StaticStrings::#static_string_ident => {
            __key.drop_with_heap(heap);
            if #slot.is_some() {
                __value.drop_with_heap(heap);
                __cleanup!(crate::exception_private::ExcType::type_error_positional_keyword_conflict(
                    #func_name,
                    #field_name_lit,
                    #pos,
                ));
            }
            match <#ty as crate::args::FromValue>::from_value(__value, heap, interns) {
                ::std::result::Result::Ok(__v) => {
                    #slot = ::std::option::Option::Some(__v);
                }
                ::std::result::Result::Err(__e) => {
                    __cleanup!(__e);
                }
            }
        }
    }
}

fn kwarg_arm_kw_only(field: &Field, slot: &Ident, func_name: &str) -> TokenStream {
    let static_string_ident = field.static_string_variant();
    let ty = &field.ty;
    let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
    quote! {
        ::std::option::Option::Some(__id) if __id == crate::intern::StaticStrings::#static_string_ident => {
            __key.drop_with_heap(heap);
            if #slot.is_some() {
                __value.drop_with_heap(heap);
                __cleanup!(crate::exception_private::ExcType::type_error_multiple_values(
                    #func_name,
                    #field_name_lit,
                ));
            }
            match <#ty as crate::args::FromValue>::from_value(__value, heap, interns) {
                ::std::result::Result::Ok(__v) => {
                    #slot = ::std::option::Option::Some(__v);
                }
                ::std::result::Result::Err(__e) => {
                    __cleanup!(__e);
                }
            }
        }
    }
}

impl Field {
    /// `StaticStrings::PascalCase(ident)` — or the override from `static_string = "..."`.
    fn static_string_variant(&self) -> Ident {
        if let Some(explicit) = &self.static_string {
            explicit.clone()
        } else {
            let pascal = snake_to_pascal(&self.ident.to_string());
            Ident::new(&pascal, self.ident.span())
        }
    }
}

fn snake_to_pascal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = true;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn vec_element_ty(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else { return None };
    let last = type_path.path.segments.last()?;
    if last.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    })
}

/// Parse the `#[from_args(...)]` attributes attached to the struct itself.
fn parse_struct_attrs(attrs: &[syn::Attribute]) -> syn::Result<(String, AtMostStyle)> {
    let mut name: Option<String> = None;
    let mut at_most_style = AtMostStyle::Standard;
    for attr in attrs {
        if !attr.path().is_ident("from_args") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("at_most_positional") {
                at_most_style = AtMostStyle::Positional;
                Ok(())
            } else {
                Err(meta.error("unknown struct attribute; expected `name = \"...\"` or `at_most_positional`"))
            }
        })?;
    }
    let name = name.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "missing `#[from_args(name = \"...\")]` on the struct",
        )
    })?;
    Ok((name, at_most_style))
}

#[derive(Default)]
struct FieldAttrs {
    kind: FieldKind,
    default: Option<DefaultExpr>,
    static_string: Option<Ident>,
}

fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    let mut seen_role = false;
    for attr in attrs {
        if !attr.path().is_ident("from_args") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let set_role = |out: &mut FieldAttrs, kind: FieldKind, seen: &mut bool| {
                if *seen {
                    return Err(meta.error("only one of `pos_only`, `kw_only`, `varargs`, `varkwargs` may be set"));
                }
                out.kind = kind;
                *seen = true;
                Ok(())
            };

            if meta.path.is_ident("pos_only") {
                set_role(&mut out, FieldKind::PosOnly, &mut seen_role)
            } else if meta.path.is_ident("kw_only") {
                set_role(&mut out, FieldKind::KwOnly, &mut seen_role)
            } else if meta.path.is_ident("varargs") {
                set_role(&mut out, FieldKind::Varargs, &mut seen_role)
            } else if meta.path.is_ident("varkwargs") {
                set_role(&mut out, FieldKind::Varkwargs, &mut seen_role)
            } else if meta.path.is_ident("default") {
                if out.default.is_some() {
                    return Err(meta.error("duplicate `default` attribute"));
                }
                // Support both bare `default` and `default = <expr>`.
                if meta.input.peek(Token![=]) {
                    let _: Token![=] = meta.input.parse()?;
                    let expr: Expr = meta.input.parse()?;
                    out.default = Some(DefaultExpr::Explicit(Box::new(expr)));
                } else {
                    out.default = Some(DefaultExpr::DefaultTrait);
                }
                Ok(())
            } else if meta.path.is_ident("static_string") {
                let value: LitStr = meta.value()?.parse()?;
                out.static_string = Some(Ident::new(&value.value(), value.span()));
                Ok(())
            } else {
                Err(meta.error("unknown field attribute"))
            }
        })?;
    }
    Ok(out)
}
