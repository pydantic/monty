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
    struct_ident: Ident,
    /// Function name embedded in error messages (the `{name}()` prefix).
    func_name: String,
    /// Fields in declaration order — also the positional dispatch order.
    fields: Vec<Field>,
    varargs_idx: Option<usize>,
    varkwargs_idx: Option<usize>,
    error_style: ErrorStyle,
    /// Pre-count `positional + kwarg` and raise "takes at most M arguments
    /// (N given)" before per-arg dispatch — matches CPython's
    /// `PyArg_ParseTupleAndKeywords`. Incompatible with `varargs`/`varkwargs`.
    at_most_total: bool,
    /// Required positional count equals maximum — emits
    /// `{name} expected N argument(s), got M` (CPython `PyArg_UnpackTuple`
    /// wording). For exact-arity callables like `sorted()`.
    expected_exact: bool,
    /// Override for the function name in the unknown-kwarg error only.
    /// Used by `json.dumps`, which forwards unmatched kwargs to
    /// `JSONEncoder.__init__` and so reports that name instead.
    kwarg_error_name: Option<String>,
    /// Wrap `FromValue` errors in CPython's `_PyArg_BadArgument` wording —
    /// `{name}() argument {pos|'arg'} must be {expected}, not {got}` —
    /// for fields whose type sets `EXPECTED_TYPE_NAME`.
    bad_arg: Option<BadArgStyle>,
    /// Reject any kwarg up front with
    /// `NotImplementedError: {name}() does not yet support keyword arguments`.
    /// Migration aid for functions like `asyncio.gather` whose CPython
    /// signatures accept kwargs Monty hasn't plumbed through yet.
    /// Incompatible with `kw_only`, `varkwargs`, `kwarg_error_name`.
    kwargs_not_supported_yet: bool,
}

/// `_PyArg_BadArgument` wording shape. CPython splits between positional
/// (`strftime` and other `_PyArg_ParseTuple` callers) and named
/// (`open`, `str.encode`, `bytes.decode`, …).
#[derive(Clone, Copy)]
enum BadArgStyle {
    /// `{name}() argument {pos} must be {expected}, not {got}`.
    Positional,
    /// `{name}() argument '{arg_name}' must be {expected}, not {got}`.
    Named,
}

/// CPython error-wording family. Pure-Python (default) for `def`-defined
/// functions and most builtin methods; `C` for `PyArg_ParseTupleAndKeywords`
/// callers using the anonymous `"function"` label (e.g. `datetime`); `NamedC`
/// for C constructors that embed the name (e.g. `timezone`).
#[derive(Clone, Copy)]
enum ErrorStyle {
    /// Default. `{name}() got an unexpected keyword argument 'X'`, etc.
    Python,
    /// `this function got an unexpected keyword argument 'X'`, etc. Inner
    /// [`AtMostStyle`] picks between standard and `… positional arguments`
    /// wording for too-many errors.
    C(AtMostStyle),
    /// Like `Python` for unknown-kwarg, like `C` for arity/conflict — with
    /// `{name}()` substituted for the anonymous `function` descriptor.
    NamedC,
}

/// Wording of the "too many args" message under [`ErrorStyle::C`].
#[derive(Clone, Copy)]
enum AtMostStyle {
    /// `function takes at most M arguments (N given)` — e.g. `date`.
    Standard,
    /// `function takes at most M positional arguments (N given)` — used by
    /// constructors that want to disambiguate from kwargs, e.g. `datetime`.
    Positional,
}

/// A single field of a `FromArgs` struct.
struct Field {
    ident: Ident,
    ty: Type,
    kind: FieldKind,
    /// `None` = required.
    default: Option<DefaultExpr>,
    /// Override for the `StaticStrings` variant used in kwarg dispatch.
    static_string: Option<Ident>,
    /// 1-indexed slot in the positional-or-keyword region, used by
    /// "pos N" error messages. `None` for kw_only / varargs / varkwargs.
    pos_index: Option<usize>,
}

/// Role of a field in the signature.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FieldKind {
    #[default]
    PosOrKeyword,
    PosOnly,
    KwOnly,
    /// `*args` — collects remaining positionals.
    Varargs,
    /// `**kwargs` — collects unmatched kwargs.
    Varkwargs,
}

/// Source of a field's default value.
pub(crate) enum DefaultExpr {
    /// `#[from_args(default)]` — `Default::default()`.
    DefaultTrait,
    /// `#[from_args(default = <expr>)]`.
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

        let StructAttrs {
            name: func_name,
            error_style,
            at_most_total,
            expected_exact,
            kwarg_error_name,
            bad_arg,
            kwargs_not_supported_yet,
        } = parse_struct_attrs(attrs)?;

        let mut fields = Vec::with_capacity(named.named.len());
        let mut varargs_idx = None;
        let mut varkwargs_idx = None;

        for field in &named.named {
            let opts = parse_field_attrs(&field.attrs)?;
            let ident = field.ident.clone().expect("named field");
            // `kind` may still be patched in the second pass — e.g. a
            // `PosOrKeyword` after a varargs becomes implicit `kw_only`.
            fields.push(Field {
                ident,
                ty: field.ty.clone(),
                kind: opts.kind,
                default: opts.default,
                static_string: opts.static_string,
                pos_index: None,
            });
        }

        // Second pass: resolve implicit kw_only-after-varargs, enforce field
        // ordering, assign 1-based positional indices, locate varargs slots.
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

        if at_most_total && (varargs_idx.is_some() || varkwargs_idx.is_some()) {
            return Err(syn::Error::new(
                struct_ident.span(),
                "`at_most_total` cannot be combined with `varargs` or `varkwargs` \
                 — the up-front total-count check is only meaningful for \
                 signatures with a fixed maximum",
            ));
        }
        if expected_exact && (varargs_idx.is_some() || at_most_total) {
            return Err(syn::Error::new(
                struct_ident.span(),
                "`expected_exact` cannot be combined with `varargs` or `at_most_total` \
                 — the exact-arity wording assumes a single fixed required positional count",
            ));
        }
        if kwargs_not_supported_yet {
            if varkwargs_idx.is_some() {
                return Err(syn::Error::new(
                    struct_ident.span(),
                    "`kwargs_not_supported_yet` cannot be combined with `varkwargs` \
                     — the flag rejects every kwarg up front, so there's nothing to collect",
                ));
            }
            if fields.iter().any(|f| matches!(f.kind, FieldKind::KwOnly)) {
                return Err(syn::Error::new(
                    struct_ident.span(),
                    "`kwargs_not_supported_yet` cannot be combined with `kw_only` fields \
                     — the flag rejects every kwarg up front, so kw_only slots are unreachable",
                ));
            }
            if kwarg_error_name.is_some() {
                return Err(syn::Error::new(
                    struct_ident.span(),
                    "`kwargs_not_supported_yet` cannot be combined with `kwarg_error_name` \
                     — the override only applies to the unknown-kwarg dispatch path, which is skipped",
                ));
            }
        }

        Ok(Self {
            struct_ident: struct_ident.clone(),
            func_name,
            fields,
            varargs_idx,
            varkwargs_idx,
            error_style,
            at_most_total,
            expected_exact,
            kwarg_error_name,
            bad_arg,
            kwargs_not_supported_yet,
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
        let slot_decls = self.render_slot_decls(&slots);
        let cleanup_block = self.render_cleanup_block(&slots);
        let no_kwargs_check = self.render_no_kwargs_check();
        let total_check = self.render_total_check(max_positional);
        let exact_check = self.render_expected_exact_check();
        let at_least_check = self.render_at_least_positional_check();
        let positional_loop = self.render_positional_loop(&slots, max_positional, has_varargs);
        let unknown_decl = self.render_unknown_kwarg_decl();
        let kwarg_loop = self.render_kwarg_loop(&slots);
        let missing_check = self.render_missing_required_check(&slots);
        let unknown_check = self.render_unknown_kwarg_check();
        let build_struct = self.render_build_struct(&slots);

        quote! {
            #[automatically_derived]
            impl #struct_ident {
                /// Extract arguments into `Self`. On any error path, every
                /// already-extracted heap value is dropped via `DropWithHeap`
                /// so refcounts stay balanced.
                pub(crate) fn from_args(
                    args: crate::args::ArgValues,
                    vm: &mut crate::bytecode::VM<'_, impl crate::resource::ResourceTracker>,
                ) -> crate::exception_private::RunResult<Self> {
                    use crate::args::FromValue as _; // allow local import
                    use crate::heap::DropWithHeap as _; // allow local import

                    let (mut __pos_iter, __kwargs_holder) = args.into_parts();
                    let mut __kwargs_iter = __kwargs_holder.into_iter();

                    #slot_decls

                    // Drops every owning slot + both iterators on the error
                    // path. Inlined so it captures every slot ident by name.
                    macro_rules! __cleanup {
                        ($err:expr) => {{
                            #cleanup_block
                            // Also drop anything left in the iterators.
                            __pos_iter.drop_with_heap(vm);
                            __kwargs_iter.drop_with_heap(vm);
                            return Err($err);
                        }};
                    }

                    #no_kwargs_check
                    #total_check
                    #exact_check
                    #at_least_check
                    #unknown_decl
                    #positional_loop
                    #kwarg_loop
                    #missing_check
                    #unknown_check

                    #build_struct
                }
            }
        }
    }

    /// Pre-check for `kwargs_not_supported_yet`. Raises
    /// `NotImplementedError: {name}() does not yet support keyword arguments`
    /// before any positional dispatch — deliberately distinct from CPython's
    /// `TypeError: takes no keyword arguments` so the "Monty TODO" intent
    /// reads clearly at the error site.
    fn render_no_kwargs_check(&self) -> TokenStream {
        if !self.kwargs_not_supported_yet {
            return TokenStream::new();
        }
        let func_name = self.func_name.as_str();
        quote! {
            if ::std::iter::ExactSizeIterator::len(&__kwargs_iter) > 0 {
                __cleanup!(crate::exception_private::ExcType::kwargs_not_implemented(#func_name));
            }
        }
    }

    /// Pre-check for `at_most_total`: counts pos+kwargs and raises
    /// "takes at most M arguments (N given)" before any extraction, to match
    /// CPython's `PyArg_ParseTupleAndKeywords` wording.
    fn render_total_check(&self, max_positional: usize) -> TokenStream {
        if !self.at_most_total {
            return TokenStream::new();
        }
        // The pre-check uses different helpers from the per-arg "at most":
        // C-style stays on `type_error_c_at_most*` (`date` reports
        // `function takes at most 3 arguments (4 given)`); Python and NamedC
        // both pivot to the parenthesised method form
        // (`str.expandtabs() takes at most 1 argument (2 given)`).
        let func_name = self.func_name.as_str();
        let err_expr = match self.error_style {
            ErrorStyle::C(AtMostStyle::Standard) => quote! {
                crate::exception_private::ExcType::type_error_c_at_most(#max_positional, __total)
            },
            ErrorStyle::C(AtMostStyle::Positional) => quote! {
                crate::exception_private::ExcType::type_error_c_at_most_positional(#max_positional, __total)
            },
            ErrorStyle::Python | ErrorStyle::NamedC => quote! {
                crate::exception_private::ExcType::type_error_method_at_most(#func_name, #max_positional, __total)
            },
        };
        quote! {
            {
                let __total = ::std::iter::ExactSizeIterator::len(&__pos_iter)
                    + ::std::iter::ExactSizeIterator::len(&__kwargs_iter);
                if __total > #max_positional {
                    __cleanup!(#err_expr);
                }
            }
        }
    }

    /// Build the "too many positional args" error expression for the current
    /// style. Centralised so the positional loop and its zero-arg fast path
    /// stay in sync.
    fn at_most_err_expr(&self, max_lit: usize, actual: &TokenStream) -> TokenStream {
        let func_name = self.func_name.as_str();
        if self.expected_exact {
            // Pre-check should already have fired; emit matching wording anyway.
            return quote! {
                crate::exception_private::ExcType::type_error_expected_exact(#func_name, #max_lit, #actual)
            };
        }
        if self.use_c_method_arity_wording() {
            // Required pos-only fields → CPython C-method wording,
            // e.g. `replace() takes at most 3 arguments (4 given)`.
            return quote! {
                crate::exception_private::ExcType::type_error_method_at_most(#func_name, #max_lit, #actual)
            };
        }
        match self.error_style {
            ErrorStyle::C(AtMostStyle::Standard) => quote! {
                crate::exception_private::ExcType::type_error_c_at_most(#max_lit, #actual)
            },
            ErrorStyle::C(AtMostStyle::Positional) => {
                // CPython pivots from "M positional arguments" to "M_total
                // arguments" once the overflow exceeds positional + kw-only.
                let max_total = max_lit + self.kw_only_count();
                quote! {
                    crate::exception_private::ExcType::type_error_c_at_most_positional_or_total(
                        #max_lit, #max_total, #actual,
                    )
                }
            }
            ErrorStyle::NamedC => quote! {
                crate::exception_private::ExcType::type_error_method_at_most(#func_name, #max_lit, #actual)
            },
            ErrorStyle::Python => quote! {
                crate::exception_private::ExcType::type_error_at_most(#func_name, #max_lit, #actual)
            },
        }
    }

    fn named_positional_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword))
            .count()
    }

    /// Trailing keyword-only slot count — used by
    /// `type_error_c_at_most_positional_or_total` for its wording pivot.
    fn kw_only_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::KwOnly))
            .count()
    }

    /// Number of positional-region fields without a default.
    fn required_positional_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) && f.default.is_none())
            .count()
    }

    /// Required positional-only count. Non-zero → CPython's C-method
    /// `_PyArg_UnpackKeywords` wording (an "at least M positional" pre-check
    /// and a method-style "at most M" too-many error), matching `str.replace`
    /// etc. whose required args cannot be filled by kwargs.
    fn required_pos_only_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly) && f.default.is_none())
            .count()
    }

    /// Suppressed under `expected_exact`, whose own check covers the at-least
    /// direction with different wording.
    fn use_c_method_arity_wording(&self) -> bool {
        !self.expected_exact && self.required_pos_only_count() > 0
    }

    /// Pre-check for `expected_exact`: exactly `required_positional_count()`
    /// positionals, kwargs ignored (CPython's `PyArg_UnpackTuple` semantics —
    /// kwargs cannot satisfy required positionals).
    fn render_expected_exact_check(&self) -> TokenStream {
        if !self.expected_exact {
            return TokenStream::new();
        }
        let func_name = self.func_name.as_str();
        let required = self.required_positional_count();
        quote! {
            {
                let __pos_actual = ::std::iter::ExactSizeIterator::len(&__pos_iter);
                if __pos_actual != #required {
                    __cleanup!(
                        crate::exception_private::ExcType::type_error_expected_exact(
                            #func_name, #required, __pos_actual,
                        )
                    );
                }
            }
        }
    }

    /// Declare the `__unknown_kwarg` slot used to defer the first unknown
    /// kwarg's name. Only emitted under C / NamedC styles, which delay the
    /// error until after missing-required has had a chance to fire.
    fn render_unknown_kwarg_decl(&self) -> TokenStream {
        if !self.defer_unknown_kwarg() || self.varkwargs_idx.is_some() || self.kwargs_not_supported_yet {
            return TokenStream::new();
        }
        quote! {
            let mut __unknown_kwarg: ::std::option::Option<::std::string::String> =
                ::std::option::Option::None;
        }
    }

    /// Missing-required check, run *after* the kwarg loop has filled what it
    /// can. Raises the same error as the final-build path but earlier, so
    /// CPython's "missing-required before unknown-kwarg" ordering holds.
    fn render_missing_required_check(&self, slots: &[Ident]) -> TokenStream {
        if !self.defer_unknown_kwarg() || self.kwargs_not_supported_yet {
            return TokenStream::new();
        }
        let func_name = self.func_name.as_str();
        let checks = self.fields.iter().zip(slots).filter_map(|(field, slot)| {
            if !matches!(field.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) || field.default.is_some() {
                return None;
            }
            let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
            let pos = field.pos_index.unwrap_or(0);
            let missing_expr = match self.error_style {
                ErrorStyle::C(_) => quote! {
                    crate::exception_private::ExcType::type_error_c_missing_required(#field_name_lit, #pos)
                },
                ErrorStyle::NamedC => quote! {
                    crate::exception_private::ExcType::type_error_c_missing_required_named(
                        #func_name, #field_name_lit, #pos,
                    )
                },
                ErrorStyle::Python => quote! {
                    crate::exception_private::ExcType::type_error_missing_positional_with_names(
                        #func_name, &[#field_name_lit],
                    )
                },
            };
            Some(quote! {
                if #slot.is_none() {
                    __cleanup!(#missing_expr);
                }
            })
        });
        quote! { #(#checks)* }
    }

    /// Deferred unknown-kwarg check. Fires only when every required field
    /// was satisfied yet a kwarg name didn't match anything — matches
    /// CPython's `PyArg_ParseTupleAndKeywords` ordering.
    fn render_unknown_kwarg_check(&self) -> TokenStream {
        if !self.defer_unknown_kwarg() || self.varkwargs_idx.is_some() || self.kwargs_not_supported_yet {
            return TokenStream::new();
        }
        let func_name = self.func_name.as_str();
        let err_expr = match self.error_style {
            ErrorStyle::C(_) => quote! {
                crate::exception_private::ExcType::type_error_c_unexpected_keyword(&__name)
            },
            ErrorStyle::Python | ErrorStyle::NamedC => quote! {
                crate::exception_private::ExcType::type_error_unexpected_keyword(#func_name, &__name)
            },
        };
        quote! {
            if let ::std::option::Option::Some(__name) = __unknown_kwarg.take() {
                __cleanup!(#err_expr);
            }
        }
    }

    /// C-method "at least M positional" pre-check, used when there are
    /// required pos-only fields (which kwargs cannot satisfy). Raises e.g.
    /// `replace() takes at least 2 positional arguments (1 given)`.
    fn render_at_least_positional_check(&self) -> TokenStream {
        if !self.use_c_method_arity_wording() {
            return TokenStream::new();
        }
        let func_name = self.func_name.as_str();
        let required = self.required_pos_only_count();
        quote! {
            {
                let __pos_actual = ::std::iter::ExactSizeIterator::len(&__pos_iter);
                if __pos_actual < #required {
                    __cleanup!(
                        crate::exception_private::ExcType::type_error_at_least_positional(
                            #func_name, #required, __pos_actual,
                        )
                    );
                }
            }
        }
    }

    fn render_slot_decls(&self, slots: &[Ident]) -> TokenStream {
        let decls = self.fields.iter().zip(slots).map(|(field, slot)| {
            let ty = &field.ty;
            match field.kind {
                FieldKind::Varargs => {
                    let elem = vec_element_ty(ty).unwrap_or_else(|| ty.clone());
                    quote! {
                        let mut #slot: ::std::vec::Vec<#elem> = ::std::vec::Vec::new();
                    }
                }
                FieldKind::Varkwargs => quote! {
                    let mut #slot: ::std::vec::Vec<(
                        crate::intern::StringId,
                        crate::value::Value,
                    )> = ::std::vec::Vec::new();
                },
                _ => {
                    // `Option<T>` so we can distinguish absent from present
                    // (drives default fallback and duplicate detection).
                    quote! {
                        let mut #slot: ::std::option::Option<#ty> = ::std::option::Option::None;
                    }
                }
            }
        });
        quote! { #(#decls)* }
    }

    fn render_cleanup_block(&self, slots: &[Ident]) -> TokenStream {
        let drops = self.fields.iter().zip(slots).map(|(field, slot)| match field.kind {
            FieldKind::Varargs => {
                quote! {
                    let __taken = ::std::mem::take(&mut #slot);
                    __taken.drop_with_heap(vm);
                }
            }
            FieldKind::Varkwargs => {
                quote! {
                    for (_, __v) in ::std::mem::take(&mut #slot) {
                        __v.drop_with_heap(vm);
                    }
                }
            }
            _ => {
                let ty = &field.ty;
                quote! {
                    if let ::std::option::Option::Some(__v) = #slot.take() {
                        <#ty as crate::args::FromValue>::drop_extracted(__v, vm);
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
            let arg_ident = format_ident!("__arg");
            let pos = field.pos_index.unwrap_or(arm_idx + 1);
            let arg_name = field.ident.to_string();
            let extract = self.render_from_value_call(ty, slot, pos, &arg_name, &arg_ident);
            arms.push(quote! {
                #arm_idx_lit => { #extract }
            });
            arm_idx += 1;
        }

        // Zero positional slots, no varargs: collapse to a single overflow
        // check. The full while/match would warn about the dead `+= 1`.
        if max_positional == 0 && !has_varargs {
            let err_expr = self.at_most_err_expr(0, &quote!(__actual));
            return quote! {
                if let ::std::option::Option::Some(__arg) = ::std::iter::Iterator::next(&mut __pos_iter) {
                    __arg.drop_with_heap(vm);
                    let __actual = 1
                        + ::std::iter::ExactSizeIterator::len(&__pos_iter)
                        + ::std::iter::ExactSizeIterator::len(&__kwargs_iter);
                    __cleanup!(#err_expr);
                }
            };
        }

        // Tail: either dispatch into varargs, or raise "at most N".
        let tail = if let Some(varargs_idx) = self.varargs_idx {
            let varargs_slot = &slots[varargs_idx];
            let elem_ty =
                vec_element_ty(&self.fields[varargs_idx].ty).unwrap_or_else(|| self.fields[varargs_idx].ty.clone());
            quote! {
                _ => {
                    match <#elem_ty as crate::args::FromValue>::from_value(__arg, vm) {
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
            let err_expr = self.at_most_err_expr(max_positional, &quote!(__actual));
            quote! {
                _ => {
                    // Drop the unconsumed arg ourselves; include remaining
                    // positionals *and* kwargs in `__actual` so the count
                    // matches CPython's "(M given)" total. `__cleanup!`
                    // drains both iterators.
                    __arg.drop_with_heap(vm);
                    let __actual = __pos_count
                        + 1
                        + ::std::iter::ExactSizeIterator::len(&__pos_iter)
                        + ::std::iter::ExactSizeIterator::len(&__kwargs_iter);
                    __cleanup!(#err_expr);
                }
            }
        };

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

    /// `FromValue::from_value` call that fills `slot`. Used by both the
    /// positional loop (`value_var = __arg`) and the kwarg arms
    /// (`value_var = __value`) so `encode(42)` and `encode(encoding=42)`
    /// report identical errors. When `bad_arg` is set, wraps the inner
    /// error in CPython's `_PyArg_BadArgument` wording.
    fn render_from_value_call(
        &self,
        ty: &Type,
        slot: &Ident,
        pos: usize,
        arg_name: &str,
        value_var: &Ident,
    ) -> TokenStream {
        let Some(style) = self.bad_arg else {
            return quote! {
                match <#ty as crate::args::FromValue>::from_value(#value_var, vm) {
                    ::std::result::Result::Ok(__v) => {
                        #slot = ::std::option::Option::Some(__v);
                    }
                    ::std::result::Result::Err(__e) => {
                        __cleanup!(__e);
                    }
                }
            };
        };
        let func_name = self.func_name.as_str();
        let bad_arg_err = match style {
            BadArgStyle::Positional => quote! {
                crate::exception_private::ExcType::type_error_bad_arg_pos(
                    #func_name,
                    #pos,
                    __expected,
                    __got.cpython_arg_name(),
                )
            },
            BadArgStyle::Named => quote! {
                crate::exception_private::ExcType::type_error_bad_arg_named(
                    #func_name,
                    #arg_name,
                    __expected,
                    __got.cpython_arg_name(),
                )
            },
        };
        quote! {
            {
                // Snapshot the type *before* `from_value` consumes the value
                // (the error path no longer has access to it). Skip the
                // lookup when the field type has no CPython label.
                let __got_type =
                    if <#ty as crate::args::FromValue>::EXPECTED_TYPE_NAME.is_some() {
                        ::std::option::Option::Some(#value_var.py_type_heap(vm.heap))
                    } else {
                        ::std::option::Option::None
                    };
                match <#ty as crate::args::FromValue>::from_value(#value_var, vm) {
                    ::std::result::Result::Ok(__v) => {
                        #slot = ::std::option::Option::Some(__v);
                    }
                    ::std::result::Result::Err(__e) => {
                        match (
                            <#ty as crate::args::FromValue>::EXPECTED_TYPE_NAME,
                            __got_type,
                        ) {
                            (
                                ::std::option::Option::Some(__expected),
                                ::std::option::Option::Some(__got),
                            ) => __cleanup!(#bad_arg_err),
                            _ => __cleanup!(__e),
                        }
                    }
                }
            }
        }
    }

    /// True when C / NamedC styles need to defer unknown-kwarg errors until
    /// after the missing-required check, matching CPython's
    /// `PyArg_ParseTupleAndKeywords` order. Python style errors immediately.
    fn defer_unknown_kwarg(&self) -> bool {
        matches!(self.error_style, ErrorStyle::C(_) | ErrorStyle::NamedC)
    }

    fn render_kwarg_loop(&self, slots: &[Ident]) -> TokenStream {
        if self.kwargs_not_supported_yet {
            // Pre-check already rejected any kwarg; skip the loop entirely.
            return TokenStream::new();
        }
        let mut arms: Vec<TokenStream> = Vec::new();
        for (field, slot) in self.fields.iter().zip(slots) {
            let arm = match field.kind {
                FieldKind::PosOnly | FieldKind::Varargs | FieldKind::Varkwargs => continue,
                FieldKind::PosOrKeyword => self.kwarg_arm_pos_or_kw(field, slot),
                FieldKind::KwOnly => self.kwarg_arm_kw_only(field, slot),
            };
            arms.push(arm);
        }

        let defer_unknown = self.defer_unknown_kwarg();

        let unknown_arm = if let Some(varkwargs_idx) = self.varkwargs_idx {
            let varkwargs_slot = &slots[varkwargs_idx];
            quote! {
                let Some(__id) = __key_str.string_id() else {
                    // TODO: intern heap-string keys via `Interns` instead of rejecting.
                    __value.drop_with_heap(vm);
                    __key.drop_with_heap(vm);
                    __cleanup!(crate::exception_private::ExcType::type_error_kwargs_nonstring_key());
                };
                __key.drop_with_heap(vm);
                #varkwargs_slot.push((__id, __value));
            }
        } else if defer_unknown {
            // Stash first unknown key; emit it later only if every required
            // field was filled. See `defer_unknown_kwarg`.
            quote! {
                __value.drop_with_heap(vm);
                if __unknown_kwarg.is_none() {
                    __unknown_kwarg = ::std::option::Option::Some(__key_str.as_str(vm.interns).to_owned());
                }
                __key.drop_with_heap(vm);
            }
        } else {
            // `json.dumps` uses `kwarg_error_name` to report
            // `JSONEncoder.__init__()` here while arity errors keep `dumps`.
            let func_name = self.kwarg_error_name.as_deref().unwrap_or(self.func_name.as_str());
            quote! {
                __value.drop_with_heap(vm);
                let __unexpected = __key_str.as_str(vm.interns).to_owned();
                __key.drop_with_heap(vm);
                __cleanup!(crate::exception_private::ExcType::type_error_unexpected_keyword(#func_name, &__unexpected));
            }
        };

        // Pos-only kwarg rejection arms — only emitted when the field has an
        // explicit `static_string` override, so we know the dispatch variant
        // exists. Without one, the kwarg falls through to "unexpected"
        // instead of the CPython "positional-only passed as keyword" form.
        let mut pos_only_arms: Vec<TokenStream> = Vec::new();
        for field in &self.fields {
            if matches!(field.kind, FieldKind::PosOnly) && field.static_string.is_some() {
                let key_id_expr = field.kwarg_string_id_expr();
                let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
                let func_name = &self.func_name;
                pos_only_arms.push(quote! {
                    if __key_str.matches(#key_id_expr, vm.interns) {
                        __value.drop_with_heap(vm);
                        __key.drop_with_heap(vm);
                        __cleanup!(crate::exception_private::ExcType::type_error_positional_only(#func_name, #field_name_lit));
                    } else
                });
            }
        }

        // Each arm trails an `else` so they chain; the final block handles
        // unknown kwargs or **varkwargs collection.
        quote! {
            while let ::std::option::Option::Some((__key, __value)) = ::std::iter::Iterator::next(&mut __kwargs_iter) {
                let ::std::option::Option::Some(__key_str) = __key.as_either_str(vm.heap) else {
                    __value.drop_with_heap(vm);
                    __key.drop_with_heap(vm);
                    __cleanup!(crate::exception_private::ExcType::type_error_kwargs_nonstring_key());
                };
                #(#pos_only_arms)*
                #(#arms)*
                {
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
                        // Empty vec collapses to `Empty` for cheap caller checks.
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
                            let missing_expr = match self.error_style {
                                ErrorStyle::C(_) => quote! {
                                    crate::exception_private::ExcType::type_error_c_missing_required(#field_name_lit, #pos)
                                },
                                ErrorStyle::NamedC => quote! {
                                    crate::exception_private::ExcType::type_error_c_missing_required_named(
                                        #func_name,
                                        #field_name_lit,
                                        #pos,
                                    )
                                },
                                ErrorStyle::Python => quote! {
                                    crate::exception_private::ExcType::type_error_missing_positional_with_names(
                                        #func_name,
                                        &[#field_name_lit],
                                    )
                                },
                            };
                            quote! {
                                #ident: match #slot.take() {
                                    ::std::option::Option::Some(__v) => __v,
                                    ::std::option::Option::None => {
                                        __cleanup!(#missing_expr);
                                    }
                                },
                            }
                        } else {
                            // Required keyword-only argument.
                            quote! {
                                #ident: match #slot.take() {
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
                        #ident: #slot.take().unwrap_or_default(),
                    },
                    Some(DefaultExpr::Explicit(expr)) => quote! {
                        #ident: #slot.take().unwrap_or_else(|| { #expr }),
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

impl Signature {
    fn kwarg_arm_pos_or_kw(&self, field: &Field, slot: &Ident) -> TokenStream {
        let func_name = self.func_name.as_str();
        let key_id_expr = field.kwarg_string_id_expr();
        let ty = &field.ty;
        let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
        let pos = field.pos_index.unwrap_or(0);
        let conflict_expr = match self.error_style {
            ErrorStyle::C(_) => quote! {
                crate::exception_private::ExcType::type_error_positional_keyword_conflict(
                    #func_name,
                    #field_name_lit,
                    #pos,
                )
            },
            ErrorStyle::NamedC => {
                // `argument for timezone() given by name ('offset') and position (1)`.
                let descriptor = format!("{func_name}()");
                quote! {
                    crate::exception_private::ExcType::type_error_positional_keyword_conflict(
                        #descriptor,
                        #field_name_lit,
                        #pos,
                    )
                }
            }
            ErrorStyle::Python => quote! {
                crate::exception_private::ExcType::type_error_duplicate_arg(#func_name, #field_name_lit)
            },
        };
        let value_ident = format_ident!("__value");
        let arg_name = field.ident.to_string();
        let extract = self.render_from_value_call(ty, slot, pos, &arg_name, &value_ident);
        quote! {
            if __key_str.matches(#key_id_expr, vm.interns) {
                __key.drop_with_heap(vm);
                if #slot.is_some() {
                    __value.drop_with_heap(vm);
                    __cleanup!(#conflict_expr);
                }
                #extract
            } else
        }
    }

    fn kwarg_arm_kw_only(&self, field: &Field, slot: &Ident) -> TokenStream {
        let func_name = self.func_name.as_str();
        let key_id_expr = field.kwarg_string_id_expr();
        let ty = &field.ty;
        let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
        let value_ident = format_ident!("__value");
        // kw_only fields have no positional index; only `bad_arg` reads it,
        // and CPython `_PyArg_BadArgument` callers don't use kw_only.
        let arg_name = field.ident.to_string();
        let extract = self.render_from_value_call(ty, slot, 0, &arg_name, &value_ident);
        quote! {
            if __key_str.matches(#key_id_expr, vm.interns) {
                __key.drop_with_heap(vm);
                if #slot.is_some() {
                    __value.drop_with_heap(vm);
                    __cleanup!(crate::exception_private::ExcType::type_error_multiple_values(
                        #func_name,
                        #field_name_lit,
                    ));
                }
                #extract
            } else
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

    /// `StringId` expression used by `__key_str.matches(...)` for kwarg
    /// dispatch. Single-char ASCII field names go through the
    /// `StringId::from_ascii(0..128)` fast path — they aren't `StaticStrings`
    /// variants, so a literal `StaticStrings::A` lookup would never hit.
    fn kwarg_string_id_expr(&self) -> TokenStream {
        let name = self.ident.to_string();
        if self.static_string.is_none() && name.len() == 1 && name.is_ascii() {
            let byte = name.as_bytes()[0];
            quote! { crate::intern::StringId::from_ascii(#byte) }
        } else {
            let variant = self.static_string_variant();
            quote! { crate::intern::StringId::from(crate::intern::StaticStrings::#variant) }
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

/// Parsed `#[from_args(...)]` set on the struct itself.
struct StructAttrs {
    name: String,
    error_style: ErrorStyle,
    at_most_total: bool,
    expected_exact: bool,
    kwarg_error_name: Option<String>,
    bad_arg: Option<BadArgStyle>,
    kwargs_not_supported_yet: bool,
}

/// Parse the `#[from_args(...)]` attributes attached to the struct itself.
fn parse_struct_attrs(attrs: &[syn::Attribute]) -> syn::Result<StructAttrs> {
    let mut name: Option<String> = None;
    let mut at_most_style = AtMostStyle::Standard;
    let mut error_style = ErrorStyle::Python;
    let mut style_set = false;
    let mut is_c_style = false;
    let mut at_most_total = false;
    let mut expected_exact = false;
    let mut kwarg_error_name: Option<String> = None;
    let mut bad_arg: Option<BadArgStyle> = None;
    let mut kwargs_not_supported_yet = false;
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
            } else if meta.path.is_ident("at_most_total") {
                at_most_total = true;
                Ok(())
            } else if meta.path.is_ident("expected_exact") {
                expected_exact = true;
                Ok(())
            } else if meta.path.is_ident("kwarg_error_name") {
                let value: LitStr = meta.value()?.parse()?;
                kwarg_error_name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("bad_arg") {
                if bad_arg.is_some() {
                    return Err(meta.error("`bad_arg` and `bad_arg_named` are mutually exclusive"));
                }
                bad_arg = Some(BadArgStyle::Positional);
                Ok(())
            } else if meta.path.is_ident("bad_arg_named") {
                if bad_arg.is_some() {
                    return Err(meta.error("`bad_arg` and `bad_arg_named` are mutually exclusive"));
                }
                bad_arg = Some(BadArgStyle::Named);
                Ok(())
            } else if meta.path.is_ident("c_error") {
                if style_set {
                    return Err(meta.error("`c_error` and `c_error_named` are mutually exclusive"));
                }
                is_c_style = true;
                style_set = true;
                Ok(())
            } else if meta.path.is_ident("c_error_named") {
                if style_set {
                    return Err(meta.error("`c_error` and `c_error_named` are mutually exclusive"));
                }
                error_style = ErrorStyle::NamedC;
                style_set = true;
                Ok(())
            } else if meta.path.is_ident("kwargs_not_supported_yet") {
                kwargs_not_supported_yet = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown struct attribute; expected `name = \"...\"`, `at_most_positional`, `at_most_total`, `expected_exact`, `kwarg_error_name = \"...\"`, `bad_arg`, `bad_arg_named`, `c_error`, `c_error_named`, or `kwargs_not_supported_yet`",
                ))
            }
        })?;
    }
    let name = name.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "missing `#[from_args(name = \"...\")]` on the struct",
        )
    })?;
    // `at_most_style` only matters under `c_error`; fold it into the variant.
    if is_c_style {
        error_style = ErrorStyle::C(at_most_style);
    }
    Ok(StructAttrs {
        name,
        error_style,
        at_most_total,
        expected_exact,
        kwarg_error_name,
        bad_arg,
        kwargs_not_supported_yet,
    })
}

#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) kind: FieldKind,
    pub(crate) default: Option<DefaultExpr>,
    pub(crate) static_string: Option<Ident>,
}

pub(crate) fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
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
