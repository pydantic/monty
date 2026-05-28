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
    /// Selects the wording family used for argument-count / argument-name
    /// errors. See [`ErrorStyle`] for the three options.
    error_style: ErrorStyle,
    /// When set, the generated code pre-counts `positional + kwarg` and
    /// raises `"… takes at most M argument(s) (N given)"` *before* doing any
    /// per-arg dispatch. Matches CPython's `PyArg_ParseTupleAndKeywords`
    /// behaviour (e.g. `expandtabs(8, tabsize=4)` →
    /// `expandtabs() takes at most 1 argument (2 given)`). Mutually
    /// exclusive with `varargs`/`varkwargs` — the pre-count is meaningless
    /// when the signature accepts unbounded inputs.
    at_most_total: bool,
    /// When set, the macro replaces both the "too few" and "too many"
    /// positional error paths with CPython's `PyArg_UnpackTuple` wording:
    /// `{name} expected N argument(s), got M` (no parens, no "positional").
    /// Used for exact-arity callables like `sorted()` whose required
    /// positional count equals the maximum. Mutually exclusive with
    /// `varargs` and `at_most_total`.
    expected_exact: bool,
    /// Optional override for the function name used in the
    /// unknown-kwarg error (`{name}() got an unexpected keyword argument 'X'`).
    /// Used by `sorted()` because CPython's sorted() delegates internally
    /// to `list.sort` and surfaces sort()'s kwarg error wording, so the
    /// kwarg-name error has to read `sort()` while arity errors keep
    /// using the struct's primary `name = "sorted"`.
    kwarg_error_name: Option<String>,
    /// When set, every typed positional / pos-or-keyword field whose
    /// `FromValue::EXPECTED_TYPE_NAME` is `Some(_)` is wrapped so that a
    /// failed conversion produces CPython's `_PyArg_BadArgument` wording
    /// (`{name}() argument {pos|'name'} must be {expected}, not {got}`),
    /// including the special `None` rendering for `NoneType` values via
    /// [`Type::cpython_arg_name`]. Replaces the inner `FromValue` error
    /// (e.g. the generic `"a str is required"`) so migrating a C-extension
    /// function to the macro keeps the original wording. The variant
    /// chooses between CPython's positional (`argument N`) and named
    /// (`argument 'X'`) phrasings; CPython picks one or the other per
    /// function (`strftime` uses positional, `open`/`encode`/`decode` use
    /// named). Only fires when `EXPECTED_TYPE_NAME` is `Some(_)`; the
    /// identity `Value` impl falls through to its native error.
    bad_arg: Option<BadArgStyle>,
}

/// Variant of `_PyArg_BadArgument`-style error wording.
///
/// CPython renders bad-argument type errors in two shapes depending on which
/// internal helper produced them. `strftime` and other `_PyArg_ParseTuple`
/// callers use the positional form, while functions registered with named
/// argument tables (e.g. `open`, `str.encode`, `bytes.decode`) use the named
/// form.
#[derive(Clone, Copy)]
enum BadArgStyle {
    /// `{name}() argument {pos} must be {expected}, not {got}`.
    Positional,
    /// `{name}() argument '{arg_name}' must be {expected}, not {got}`.
    Named,
}

/// Which family of CPython error wordings the generated code should emit.
///
/// CPython exposes three distinct phrasings depending on whether the function
/// was defined in pure Python, in a C extension via
/// `PyArg_ParseTupleAndKeywords` with the anonymous "function" label
/// (e.g. `datetime`), or in a C extension with the function's own name in the
/// message (e.g. `timezone`). The macro picks helpers from
/// `exception_private` to match.
#[derive(Clone, Copy)]
enum ErrorStyle {
    /// Pure-Python / Python-method wording. Default. Matches `def`-defined
    /// functions and most builtin methods. Example unknowns:
    /// `{name}() got an unexpected keyword argument 'X'`.
    Python,
    /// Anonymous C-constructor wording. Matches CPython's `datetime` and other
    /// `PyArg_ParseTupleAndKeywords` callers that use the generic
    /// `"function"` label. Example unknowns:
    /// `this function got an unexpected keyword argument 'X'`.
    ///
    /// The inner [`AtMostStyle`] picks which `type_error_c_at_most*` helper
    /// to emit when too many positional arguments are passed — only the C
    /// style varies here; the named styles always emit the `{name}() takes
    /// at most …` helper.
    C(AtMostStyle),
    /// Named C-constructor wording. Matches CPython types like `timezone`
    /// where messages embed the constructor name. Example unknowns:
    /// `{name}() got an unexpected keyword argument 'X'` (same wording as
    /// Python-method), but conflict / missing-positional / at-most use the
    /// C phrasings prefixed with the name (e.g.
    /// `argument for {name}() given by name ('X') and position (N)`).
    NamedC,
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
pub(crate) enum FieldKind {
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
pub(crate) enum DefaultExpr {
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

        let StructAttrs {
            name: func_name,
            error_style,
            at_most_total,
            expected_exact,
            kwarg_error_name,
            bad_arg,
        } = parse_struct_attrs(attrs)?;

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

    /// Emit the up-front total-count check when `#[from_args(at_most_total)]`
    /// is set. Returns an empty token stream otherwise. Counts
    /// `positional + kwarg` slots and raises before any extraction so that
    /// the wording matches CPython's `PyArg_ParseTupleAndKeywords`:
    /// `expandtabs() takes at most 1 argument (2 given)`.
    fn render_total_check(&self, max_positional: usize) -> TokenStream {
        if !self.at_most_total {
            return TokenStream::new();
        }
        // The `at_most_total` pre-check uses CPython's
        // `PyArg_ParseTupleAndKeywords` wording, which is *not* the same as
        // the per-arg "at most" wording the styles fall back to. C style
        // ("function") stays on the C helpers (so `date` reports
        // `function takes at most 3 arguments (4 given)`); both Python and
        // NamedC use the parenthesized method form so e.g. `expandtabs`
        // reports `str.expandtabs() takes at most 1 argument (2 given)`.
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

    /// Emit the `type_error_*_at_most*` call for "too many positional args".
    ///
    /// Centralises the per-style choice so the positional loop and its
    /// zero-positional special case stay in sync. `actual` is a token stream
    /// for the runtime value (typically `__actual`).
    fn at_most_err_expr(&self, max_lit: usize, actual: &TokenStream) -> TokenStream {
        let func_name = self.func_name.as_str();
        if self.expected_exact {
            // Should be unreachable in practice (the pre-check fires first),
            // but emit the matching wording for completeness.
            return quote! {
                crate::exception_private::ExcType::type_error_expected_exact(#func_name, #max_lit, #actual)
            };
        }
        if self.use_c_method_arity_wording() {
            // Required pos-only fields put the struct into CPython's
            // C-method dispatch style. Too-many wording becomes
            // method-style "takes at most M argument(s) (N given)",
            // matching e.g. `replace() takes at most 3 arguments (4 given)`.
            return quote! {
                crate::exception_private::ExcType::type_error_method_at_most(#func_name, #max_lit, #actual)
            };
        }
        match self.error_style {
            ErrorStyle::C(AtMostStyle::Standard) => quote! {
                crate::exception_private::ExcType::type_error_c_at_most(#max_lit, #actual)
            },
            ErrorStyle::C(AtMostStyle::Positional) => {
                // CPython switches wording from "M positional arguments"
                // to "M_total arguments" once the overflow exceeds the
                // total slot count (positional + kw-only). See
                // `type_error_c_at_most_positional_or_total` for details.
                let max_total = max_lit + self.kw_only_count();
                quote! {
                    crate::exception_private::ExcType::type_error_c_at_most_positional_or_total(
                        #max_lit, #max_total, #actual,
                    )
                }
            }
            // CPython's named-C types (e.g. timezone) emit
            // `{name}() takes at most M arguments (N given)`.
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

    /// Number of trailing keyword-only slots. Used by `at_most_positional` to
    /// compute `max_total = max_positional + kw_only`, which controls the
    /// CPython wording pivot in `type_error_c_at_most_positional_or_total`.
    fn kw_only_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::KwOnly))
            .count()
    }

    /// Counts positional-region fields that have no default (i.e. they must
    /// either come in via positionals or, for `PosOrKeyword`, via a kwarg).
    /// Used by `expected_exact` to know how many positionals must actually
    /// appear.
    fn required_positional_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) && f.default.is_none())
            .count()
    }

    /// Counts required positional-only fields. When non-zero (and
    /// `expected_exact` is not set) the macro emits CPython's C-method
    /// `_PyArg_UnpackKeywords` wording family: an "at least M positional
    /// arguments" pre-check and a method-style "at most M argument(s)"
    /// too-many error. Matches `str.replace` and other C-implemented
    /// methods whose required args cannot be filled by kwargs.
    fn required_pos_only_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly) && f.default.is_none())
            .count()
    }

    /// True when [`required_pos_only_count`] is non-zero and we're not
    /// already covered by `expected_exact` (whose exact-count check
    /// subsumes the at-least direction with its own wording).
    fn use_c_method_arity_wording(&self) -> bool {
        !self.expected_exact && self.required_pos_only_count() > 0
    }

    /// Emit the `expected_exact` pre-check when set. Validates that the
    /// positional iterator has exactly `required_positional_count()` items
    /// (kwargs are ignored — CPython's `PyArg_UnpackTuple` style does not
    /// let kwargs satisfy required positionals). Raises
    /// `"{name} expected N argument(s), got M"` matching CPython's
    /// `sorted expected 1 argument, got 0` wording.
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

    /// Emit `let mut __unknown_kwarg: Option<String> = None;` when the
    /// signature defers unknown-kwarg errors (C / NamedC styles). Returns
    /// an empty stream for Python style, where unknowns error immediately.
    fn render_unknown_kwarg_decl(&self) -> TokenStream {
        if !self.defer_unknown_kwarg() || self.varkwargs_idx.is_some() {
            return TokenStream::new();
        }
        quote! {
            let mut __unknown_kwarg: ::std::option::Option<::std::string::String> =
                ::std::option::Option::None;
        }
    }

    /// Emit the deferred missing-required-positional check.
    ///
    /// Runs *after* the kwarg loop has had a chance to fill named slots
    /// from kwargs. Walks every required pos_only / pos_or_keyword field;
    /// if any are still `None`, raises the same missing-required error
    /// that `render_build_struct` would otherwise produce — but earlier,
    /// so unknown-kwarg reporting (which CPython does last) doesn't beat
    /// the missing-required error.
    fn render_missing_required_check(&self, slots: &[Ident]) -> TokenStream {
        if !self.defer_unknown_kwarg() {
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

    /// Emit the deferred-unknown-kwarg error check.
    ///
    /// Runs after both the kwarg loop and the missing-required check, so
    /// it only fires when every required field was satisfied yet a kwarg
    /// name didn't match anything. Matches CPython's
    /// `PyArg_ParseTupleAndKeywords` ordering.
    fn render_unknown_kwarg_check(&self) -> TokenStream {
        if !self.defer_unknown_kwarg() || self.varkwargs_idx.is_some() {
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

    /// Emit the C-method "at least M positional" pre-check. Fires when the
    /// struct has at least one required positional-only field and
    /// `expected_exact` is not set. Validates that the positional iterator
    /// has at least `required_pos_only_count()` items — kwargs do not
    /// satisfy required pos-only slots. Raises
    /// `"{name}() takes at least M positional argument(s) (N given)"`
    /// matching CPython's `replace() takes at least 2 positional arguments (1 given)`.
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
            let arg_ident = format_ident!("__arg");
            let pos = field.pos_index.unwrap_or(arm_idx + 1);
            let arg_name = field.ident.to_string();
            let extract = self.render_from_value_call(ty, slot, pos, &arg_name, &arg_ident);
            arms.push(quote! {
                #arm_idx_lit => { #extract }
            });
            arm_idx += 1;
        }

        // Special case: zero positional slots and no varargs. The "loop"
        // collapses to a single "any positional is too many" check — emitting
        // the full while/match here would trigger an `unreachable_code`
        // warning because `__pos_count += 1` is dead in that shape.
        if max_positional == 0 && !has_varargs {
            let err_expr = self.at_most_err_expr(0, &quote!(__actual));
            return quote! {
                if let ::std::option::Option::Some(__arg) = ::std::iter::Iterator::next(&mut __pos_iter) {
                    __arg.drop_with_heap(heap);
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
            let err_expr = self.at_most_err_expr(max_positional, &quote!(__actual));
            quote! {
                _ => {
                    // The argument itself has not yet been consumed by from_value,
                    // so drop it explicitly before bubbling the count error.
                    // Include the remaining unconsumed positionals *and* the
                    // un-iterated kwargs in `__actual` so the count matches
                    // CPython: the C-style "function takes at most N arguments
                    // (M given)" wording counts every supplied arg, not just
                    // positionals. `__cleanup!` will drain & drop both iters.
                    __arg.drop_with_heap(heap);
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

    /// Renders the `FromValue::from_value(<value_var>, ...)` call that fills
    /// `slot`, including the optional `bad_arg` wrapping that converts the
    /// inner `FromValue` error into CPython's
    /// `{name}() argument {pos|'arg_name'} must be {expected}, not {got}`
    /// wording.
    ///
    /// Used by both the positional dispatch loop (passing `__arg`) and the
    /// kwarg dispatch arms (passing `__value`) — so that `encode(encoding=42)`
    /// and `encode(42)` produce identical errors, matching CPython. When
    /// `bad_arg` is unset, falls back to the trivial extract-or-bubble-error
    /// form.
    ///
    /// `arg_name` is the field identifier as a literal (for named-style
    /// wording); `pos` is the 1-indexed positional slot (for positional-style
    /// wording).
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
                match <#ty as crate::args::FromValue>::from_value(#value_var, heap, interns) {
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
                // Capture the value's type *before* `from_value` consumes it
                // — on the error path the value is already dropped and we
                // can't peek at it. The const-conditional ensures we only pay
                // for the lookup when this field's type has a CPython-style
                // label to report.
                let __got_type =
                    if <#ty as crate::args::FromValue>::EXPECTED_TYPE_NAME.is_some() {
                        ::std::option::Option::Some(#value_var.py_type_heap(heap))
                    } else {
                        ::std::option::Option::None
                    };
                match <#ty as crate::args::FromValue>::from_value(#value_var, heap, interns) {
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

    /// Returns true when unknown-kwarg errors should be deferred until after
    /// the missing-required check, matching CPython's
    /// `PyArg_ParseTupleAndKeywords` validation order.
    ///
    /// CPython behaviour (see playground probing):
    /// - C / NamedC styles (date, datetime, timezone, …): missing-required
    ///   wins, so a call like `date(2024, 1, foo=1)` reports
    ///   "missing required argument 'day' (pos 3)" rather than
    ///   "unexpected keyword argument 'foo'".
    /// - Python style: unknown wins. `def f(x): f(foo=1)` →
    ///   `f() got an unexpected keyword argument 'foo'`.
    fn defer_unknown_kwarg(&self) -> bool {
        matches!(self.error_style, ErrorStyle::C(_) | ErrorStyle::NamedC)
    }

    fn render_kwarg_loop(&self, slots: &[Ident]) -> TokenStream {
        // Build kwarg dispatch arms — only for fields that can be passed by name.
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
        } else if defer_unknown {
            // C / NamedC styles defer unknown-kwarg errors so missing-required
            // checks can run first (matches CPython
            // `PyArg_ParseTupleAndKeywords` order). Stash the *first* unknown
            // key name and continue processing — the error is raised after
            // the kwarg loop only if all required fields are filled.
            quote! {
                __value.drop_with_heap(heap);
                if __unknown_kwarg.is_none() {
                    __unknown_kwarg = ::std::option::Option::Some(__key_str.as_str(interns).to_owned());
                }
                __key.drop_with_heap(heap);
            }
        } else {
            // `kwarg_error_name` overrides the function name used in
            // unknown-kwarg errors (used by `sorted` to emit `sort()` here
            // even though arity errors still say `sorted`).
            let func_name = self.kwarg_error_name.as_deref().unwrap_or(self.func_name.as_str());
            quote! {
                __value.drop_with_heap(heap);
                let __unexpected = __key_str.as_str(interns).to_owned();
                __key.drop_with_heap(heap);
                __cleanup!(crate::exception_private::ExcType::type_error_unexpected_keyword(#func_name, &__unexpected));
            }
        };

        // Build pos-only kwarg rejection arms — but only when the user has
        // explicitly supplied a `#[from_args(static_string = "…")]` override
        // pinning the kwarg name to a known `StaticStrings` variant. Without
        // an override we can't generate a sound runtime dispatch (the
        // auto-derived `StaticStrings::PascalCase(ident)` variant might not
        // exist), so we fall through to the generic "unexpected keyword"
        // error instead of the CPython-specific "positional-only arguments
        // passed as keyword arguments" wording.
        let mut pos_only_arms: Vec<TokenStream> = Vec::new();
        for field in &self.fields {
            if matches!(field.kind, FieldKind::PosOnly) && field.static_string.is_some() {
                let key_id_expr = field.kwarg_string_id_expr();
                let field_name_lit = LitStr::new(&field.ident.to_string(), field.ident.span());
                let func_name = &self.func_name;
                pos_only_arms.push(quote! {
                    if __key_str.matches(#key_id_expr, interns) {
                        __value.drop_with_heap(heap);
                        __key.drop_with_heap(heap);
                        __cleanup!(crate::exception_private::ExcType::type_error_positional_only(#func_name, #field_name_lit));
                    } else
                });
            }
        }

        // Glue the if/else chain together. Each arm in `arms` and
        // `pos_only_arms` ends with a trailing `else` so the next arm chains
        // cleanly; the final `else` block handles unknown kwargs or
        // **varkwargs collection.
        quote! {
            while let ::std::option::Option::Some((__key, __value)) = ::std::iter::Iterator::next(&mut __kwargs_iter) {
                let ::std::option::Option::Some(__key_str) = __key.as_either_str(heap) else {
                    __value.drop_with_heap(heap);
                    __key.drop_with_heap(heap);
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
                // Embed `{name}()` as the func_descriptor so the conflict message
                // is e.g. `argument for timezone() given by name ('offset') and
                // position (1)` (matches CPython's `timezone`).
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
            if __key_str.matches(#key_id_expr, interns) {
                __key.drop_with_heap(heap);
                if #slot.is_some() {
                    __value.drop_with_heap(heap);
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
        // kw_only fields don't have a positional index; pass 0 — only the
        // `bad_arg` path uses it, and kw_only fields rarely combine with
        // `bad_arg` (the CPython `_PyArg_BadArgument` callers don't expose
        // their args as kw_only).
        let arg_name = field.ident.to_string();
        let extract = self.render_from_value_call(ty, slot, 0, &arg_name, &value_ident);
        quote! {
            if __key_str.matches(#key_id_expr, interns) {
                __key.drop_with_heap(heap);
                if #slot.is_some() {
                    __value.drop_with_heap(heap);
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

    /// The `StringId` expression used for kwarg-name comparison in
    /// `__key_str.matches(...)`.
    ///
    /// Single-character ASCII field names (e.g. `a`, `b`) are interned via
    /// the `0..128` ASCII fast-path (`StringId::from_ascii`), not as
    /// `StaticStrings` variants. Matching against `StaticStrings::A` for
    /// such a field would never hit, so the kwarg would surface as
    /// "unexpected keyword argument". Emit `StringId::from_ascii` for
    /// those cases and `StringId::from(StaticStrings::Variant)` for
    /// everything else.
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

/// Parsed `#[from_args(...)]` attribute set attached to a struct.
///
/// Boxed up as a struct rather than a long tuple so adding new flags
/// doesn't ripple through callsite signatures — the macro grew organically
/// and the tuple was getting unwieldy.
struct StructAttrs {
    name: String,
    error_style: ErrorStyle,
    at_most_total: bool,
    expected_exact: bool,
    kwarg_error_name: Option<String>,
    bad_arg: Option<BadArgStyle>,
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
            } else {
                Err(meta.error(
                    "unknown struct attribute; expected `name = \"...\"`, `at_most_positional`, `at_most_total`, `expected_exact`, `kwarg_error_name = \"...\"`, `bad_arg`, `bad_arg_named`, `c_error`, or `c_error_named`",
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
    // `at_most_style` only matters under C-style errors; bundle it now so it
    // travels with the variant and we don't carry an unused field around.
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
