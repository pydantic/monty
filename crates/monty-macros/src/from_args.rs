//! Codegen for the `#[derive(FromArgs)]` macro.
//!
//! Parses a struct definition with `#[from_args(...)]` attributes into a
//! validated [`Signature`], then renders a thin `from_args` method: a
//! `static` `ParamSpec` describing the signature, one call to the runtime
//! binder (`crate::args::bind`, which owns all dispatch, arity, kwarg and
//! refcount-cleanup logic), and per-field `FromValue` conversions in
//! declaration order. Everything that *can* live at runtime does — the macro
//! only emits what must be compile-time: the spec, monomorphised conversion
//! calls, default expressions, and the final struct build.
//!
//! The `style` attribute names the CPython argument-parser family the target
//! function uses (see `ErrorFamily` in `crates/monty/src/args/bind_native.rs` and
//! the table in `crates/monty-macros/README.md`); it selects both error
//! wording and error ordering. Pick it by looking at how the function is
//! implemented in CPython, not by the shape of the fields.
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
    /// CPython parser family from `style = ...` (default [`Style::Clinic`]).
    style: Style,
    /// Fields in declaration order — also the positional dispatch order.
    fields: Vec<Field>,
    varargs_idx: Option<usize>,
    varkwargs_idx: Option<usize>,
    /// Pre-count `positional + kwarg` against the positional max before
    /// dispatch, reproducing `PyArg_ParseTupleAndKeywords`' total pre-check.
    /// A per-function empirical fact, not derivable from fields or style —
    /// see the litmus test in `crates/monty-macros/README.md`.
    at_most_total: bool,
    /// Override for the function name in the unknown-kwarg error only.
    /// Used by `json.dumps`, which forwards unmatched kwargs to
    /// `JSONEncoder.__init__` and so reports that name instead.
    kwarg_error_name: Option<String>,
    /// Report `FromValue` wrong-type failures in CPython's
    /// `_PyArg_BadArgument` wording — `{name}() argument {pos|'arg'} must be
    /// {expected}, not {got}` — for fields whose type sets
    /// `EXPECTED_TYPE_NAME`. Value-level failures (`FromValueFail::Raise`)
    /// surface unchanged.
    bad_arg: Option<BadArgStyle>,
    /// Reject any kwarg up front with
    /// `NotImplementedError: {name}() does not yet support keyword arguments`.
    /// Migration aid for functions like `asyncio.gather` whose CPython
    /// signatures accept kwargs Monty hasn't plumbed through yet.
    kwargs_not_supported_yet: bool,
}

/// CPython argument-parser family, set with `#[from_args(style = ...)]`.
/// Mirrors the runtime `ErrorFamily` 1:1 (the `C` pivot flag is computed from
/// the fields at render time, not spelled by the user).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// `style = def` — pure-Python `def` binding (e.g. the `re` module
    /// functions, `json.dumps`/`loads`).
    Def,
    /// Default — Argument Clinic `_PyArg_UnpackKeywords` (most modern
    /// builtins and methods).
    Clinic,
    /// `style = c` — `PyArg_ParseTupleAndKeywords` with no name in the format
    /// string; errors say `function takes …` (e.g. `date`, `datetime`).
    C,
    /// `style = c_named` — same parser with the name embedded (`:name`);
    /// errors say `{name}() …` (e.g. `timezone`, `pow`).
    CNamed,
    /// `style = unpack` — `PyArg_UnpackTuple`: fixed positional `min..max`
    /// range, `{name} expected …` wording (e.g. `unicodedata.name`). When
    /// min == max the runtime collapses to `expected N argument(s)`, so
    /// exact-arity callables use this style too.
    Unpack,
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

/// A single field of a `FromArgs` struct.
struct Field {
    ident: Ident,
    ty: Type,
    kind: FieldKind,
    /// `None` = required.
    default: Option<DefaultExpr>,
    /// Override for the `StaticStrings` variant used in kwarg dispatch.
    static_string: Option<Ident>,
    /// 1-indexed slot in the positional region, used by "pos N" error
    /// messages. `None` for kw_only / varargs / varkwargs.
    pos_index: Option<usize>,
    /// 0-indexed slot in the runtime `ParamSpec::params` array (named fields
    /// only — varargs / varkwargs own no slot).
    slot_index: Option<usize>,
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
            style,
            at_most_total,
            kwarg_error_name,
            bad_arg,
            kwargs_not_supported_yet,
        } = parse_struct_attrs(attrs)?;

        let mut fields = Vec::with_capacity(named.named.len());
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
                slot_index: None,
            });
        }

        let (varargs_idx, varkwargs_idx) = resolve_field_roles(&mut fields)?;

        let signature = Self {
            struct_ident: struct_ident.clone(),
            func_name,
            style,
            fields,
            varargs_idx,
            varkwargs_idx,
            at_most_total,
            kwarg_error_name,
            bad_arg,
            kwargs_not_supported_yet,
        };
        signature.validate()?;
        Ok(signature)
    }

    /// Style/modifier/field compatibility checks — the single place invalid
    /// combinations are rejected. Grouped per style, then the orthogonal
    /// modifiers, so each rule reads as one line of the compatibility table.
    fn validate(&self) -> syn::Result<()> {
        let err = |msg: &str| Err(syn::Error::new(self.struct_ident.span(), msg));

        match self.style {
            Style::Def => {
                if self.bad_arg.is_some() {
                    return err("`bad_arg`/`bad_arg_named` cannot be combined with `style = def` \
                         — CPython `def` binding never type-checks while binding; declare \
                         fields as raw `Value` and coerce in the function body");
                }
                if self.varargs_idx.is_some() {
                    return err("`style = def` cannot be combined with `varargs` — a `*args` \
                         signature can never raise too-many-positional, so the style has no effect");
                }
            }
            Style::Unpack => {
                if self.fields.iter().any(|f| matches!(f.kind, FieldKind::PosOrKeyword)) {
                    return err("`style = unpack` models a positional-only `PyArg_UnpackTuple` \
                         signature — every positional field must be `pos_only`");
                }
                if self.varargs_idx.is_some() || self.varkwargs_idx.is_some() {
                    return err("`style = unpack` cannot be combined with `varargs` or `varkwargs` \
                         — it models a fixed positional min..max range");
                }
            }
            Style::Clinic | Style::C | Style::CNamed => {}
        }

        if self.at_most_total {
            if matches!(self.style, Style::Def | Style::Unpack) {
                return err(
                    "`at_most_total` cannot be combined with `style = def` or `style = unpack` \
                     — the total pre-count models `PyArg_ParseTupleAndKeywords`-family C parsers",
                );
            }
            if self.varargs_idx.is_some() || self.varkwargs_idx.is_some() {
                return err("`at_most_total` cannot be combined with `varargs` or `varkwargs` \
                     — the up-front total-count check is only meaningful for \
                     signatures with a fixed maximum");
            }
        }

        if self.kwarg_error_name.is_some() && !matches!(self.style, Style::Def | Style::Clinic) {
            return err(
                "`kwarg_error_name` is only meaningful with `style = def` or the default \
                 `clinic` style — the C families defer unknown-kwarg errors past binding \
                 and `unpack` callables take no keywords worth renaming",
            );
        }

        if self.kwargs_not_supported_yet {
            if self.varkwargs_idx.is_some() {
                return err("`kwargs_not_supported_yet` cannot be combined with `varkwargs` \
                     — the flag rejects every kwarg up front, so there's nothing to collect");
            }
            if self.fields.iter().any(|f| matches!(f.kind, FieldKind::KwOnly)) {
                return err("`kwargs_not_supported_yet` cannot be combined with `kw_only` fields \
                     — the flag rejects every kwarg up front, so kw_only slots are unreachable");
            }
            if self.kwarg_error_name.is_some() {
                return err("`kwargs_not_supported_yet` cannot be combined with `kwarg_error_name` \
                     — the override only applies to the unknown-kwarg dispatch path, which is skipped");
            }
        }

        // The runtime binder's fast path fills the first `n` positional slots
        // and assumes that satisfies every required positional param — sound
        // only if required positional fields precede defaulted ones (the same
        // ordering Python enforces for `def` signatures).
        let mut seen_positional_default = false;
        for field in &self.fields {
            if !matches!(field.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) {
                continue;
            }
            if field.default.is_some() {
                seen_positional_default = true;
            } else if seen_positional_default {
                return Err(syn::Error::new(
                    field.ident.span(),
                    "required positional fields must come before positional fields with \
                     defaults — matching Python signatures, and relied on by the runtime \
                     binder's fast path",
                ));
            }
        }

        // Raw binding is deliberately separate from conversion (that split is
        // what reproduces CPython's error orderings), so `*args` elements are
        // handed over unconverted.
        if let Some(idx) = self.varargs_idx
            && !is_vec_of_value(&self.fields[idx].ty)
        {
            return Err(syn::Error::new(
                self.fields[idx].ident.span(),
                "`varargs` fields must be `Vec<Value>` — coerce elements in the function body",
            ));
        }

        // The runtime binder's fast paths return before the aggregated
        // missing-keyword check runs, so a required kw_only slot could slip
        // through binding unfilled and later surface `Bound::require`'s
        // positional wording. No current signature needs one; reject until
        // the fast paths learn to check for them.
        if let Some(field) = self
            .fields
            .iter()
            .find(|f| matches!(f.kind, FieldKind::KwOnly) && f.default.is_none())
        {
            return Err(syn::Error::new(
                field.ident.span(),
                "keyword-only fields must have a `default` — the runtime binder's fast \
                 paths skip the aggregated missing-keyword check, so a required \
                 keyword-only parameter would report the wrong error; extend the binder \
                 before allowing this",
            ));
        }

        // `default` / `static_string` describe a named parameter slot; on the
        // collector fields they would be silently dead configuration.
        for idx in [self.varargs_idx, self.varkwargs_idx].into_iter().flatten() {
            let field = &self.fields[idx];
            if field.default.is_some() || field.static_string.is_some() {
                return Err(syn::Error::new(
                    field.ident.span(),
                    "`default` and `static_string` cannot be applied to `varargs` / \
                     `varkwargs` fields — they configure a named parameter slot, which \
                     collector fields don't own",
                ));
            }
        }

        Ok(())
    }

    fn render(&self) -> TokenStream {
        let struct_ident = &self.struct_ident;
        // Dedicated owning-slots struct: holds the raw `Bound` returned by the
        // runtime binder plus one typed `Option` per named field. A `HeapGuard`
        // around it centralises error-path cleanup in one `DropWithHeap` impl,
        // so every conversion site is a plain `?`.
        let slots_struct_ident = format_ident!("__{}Slots", struct_ident);

        let named: Vec<&Field> = self.named_fields().collect();
        let n_slots = named.len();
        let slot_idents: Vec<&Ident> = named.iter().map(|f| &f.ident).collect();
        let slot_tys: Vec<&Type> = named.iter().map(|f| &f.ty).collect();

        let spec = self.render_spec(&named);
        let conversions = named.iter().map(|f| self.render_conversion(f));
        let build_fields = self.fields.iter().map(render_build_field);

        quote! {
            /// Owning slots for the corresponding `from_args`: the raw bound
            /// arguments plus each converted field. Its `DropWithHeap` impl
            /// releases both, so a `HeapGuard` around it keeps refcounts
            /// balanced on every conversion error path.
            struct #slots_struct_ident {
                raw: crate::args::Bound<#n_slots>,
                #(#slot_idents: ::std::option::Option<#slot_tys>,)*
            }

            #[automatically_derived]
            impl crate::heap::DropWithHeap for #slots_struct_ident {
                fn drop_with_heap<H: crate::heap::ContainsHeap>(self, heap: &mut H) {
                    crate::heap::DropWithHeap::drop_with_heap(self.raw, heap);
                    #(
                        if let ::std::option::Option::Some(__v) = self.#slot_idents {
                            <#slot_tys as crate::args::FromValue>::drop_extracted(__v, heap);
                        }
                    )*
                }
            }

            #[automatically_derived]
            impl #struct_ident {
                /// Extract arguments into `Self`. Binding (dispatch, arity,
                /// kwargs, cleanup) happens in `crate::args::bind`; this body
                /// only converts each raw slot in declaration order. On any
                /// error path every remaining value is dropped via the slots
                /// struct's `DropWithHeap` impl (driven by a `HeapGuard`).
                pub(crate) fn from_args(
                    args: crate::args::ArgValues,
                    vm: &mut crate::bytecode::VM<'_, impl crate::resource::ResourceTracker>,
                ) -> crate::exception_private::RunResult<Self> {
                    #spec

                    // The guard owns cleanup on every error path — including
                    // inside `bind`, which fills the slots in place. On the
                    // success path every slot is drained by the build below,
                    // so the guard's drop at scope exit is a cheap no-op.
                    let mut __guard = crate::heap::HeapGuard::new(
                        #slots_struct_ident {
                            raw: crate::args::Bound::new(&__SPEC),
                            #(#slot_idents: ::std::option::Option::None,)*
                        },
                        vm,
                    );
                    let (__slots, vm) = __guard.as_parts_mut();
                    crate::args::bind::<#n_slots>(&__SPEC, &mut __slots.raw, args, vm)?;

                    #(#conversions)*
                    __slots.raw.finish()?;

                    ::std::result::Result::Ok(Self {
                        #(#build_fields)*
                    })
                }
            }
        }
    }

    /// The `static __SPEC: ParamSpec` literal interpreted by the runtime binder.
    fn render_spec(&self, named: &[&Field]) -> TokenStream {
        let func_name = self.func_name.as_str();
        // Under `kwargs_not_supported_yet` no kwarg is ever dispatched (they
        // are all rejected up front), so no field needs a matchable id — this
        // also spares the `StaticStrings` variants those names would require.
        let params = named.iter().map(|f| f.render_param(self.kwargs_not_supported_yet));
        let family = self.render_family(named);
        let n_positional = named
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword))
            .count();
        let n_required_positional = named
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly | FieldKind::PosOrKeyword) && f.default.is_none())
            .count();
        let n_required_pos_only = named
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::PosOnly) && f.default.is_none())
            .count();
        let varargs = self.varargs_idx.is_some();
        let varkwargs = self.varkwargs_idx.is_some();
        let at_most_total = self.at_most_total;
        let kwargs_not_supported_yet = self.kwargs_not_supported_yet;
        let kwarg_error_name = if let Some(name) = &self.kwarg_error_name {
            quote! { ::std::option::Option::Some(#name) }
        } else {
            quote! { ::std::option::Option::None }
        };
        quote! {
            static __SPEC: crate::args::ParamSpec = crate::args::ParamSpec {
                func_name: #func_name,
                family: #family,
                params: &[#(#params),*],
                n_positional: #n_positional,
                n_required_positional: #n_required_positional,
                n_required_pos_only: #n_required_pos_only,
                varargs: #varargs,
                varkwargs: #varkwargs,
                at_most_total: #at_most_total,
                kwargs_not_supported_yet: #kwargs_not_supported_yet,
                kwarg_error_name: #kwarg_error_name,
            };
        }
    }

    /// The runtime `ErrorFamily` value for this style. `C`'s positional-pivot
    /// wording is derived, not spelled: CPython's `vgetargskeywords` switches
    /// to `… positional arguments …` exactly when the function has
    /// keyword-only parameters.
    fn render_family(&self, named: &[&Field]) -> TokenStream {
        match self.style {
            Style::Def => quote! { crate::args::ErrorFamily::Def },
            Style::Clinic => quote! { crate::args::ErrorFamily::Clinic },
            Style::C => {
                let pivot = named.iter().any(|f| matches!(f.kind, FieldKind::KwOnly));
                quote! { crate::args::ErrorFamily::C { positional_pivot: #pivot } }
            }
            Style::CNamed => quote! { crate::args::ErrorFamily::CNamed },
            Style::Unpack => quote! { crate::args::ErrorFamily::Unpack },
        }
    }

    /// One field's conversion: `require` (required) or `take` (defaulted) the
    /// raw slot, then `FromValue::extract_into` it into the typed slot. The
    /// per-param call order is what lets the C families interleave
    /// missing/conflict errors with conversion, exactly like CPython.
    fn render_conversion(&self, field: &Field) -> TokenStream {
        let ident = &field.ident;
        let ty = &field.ty;
        let slot_index = field.slot_index.expect("named field has a slot");
        let ctx = self.render_err_ctx(field);
        if field.default.is_none() {
            quote! {
                let __v = __slots.raw.require(#slot_index)?;
                <#ty as crate::args::FromValue>::extract_into(__v, &mut __slots.#ident, vm, #ctx)?;
            }
        } else {
            quote! {
                if let ::std::option::Option::Some(__v) = __slots.raw.take(#slot_index) {
                    <#ty as crate::args::FromValue>::extract_into(__v, &mut __slots.#ident, vm, #ctx)?;
                }
            }
        }
    }

    /// The `ArgErrCtx` literal selecting `_PyArg_BadArgument` wording for
    /// wrong-type conversion failures (see `bad_arg` on [`Signature`]).
    fn render_err_ctx(&self, field: &Field) -> TokenStream {
        let func_name = self.func_name.as_str();
        match self.bad_arg {
            None => quote! { crate::args::ArgErrCtx::Plain },
            Some(BadArgStyle::Positional) => {
                // kw_only fields have no positional index; CPython
                // `_PyArg_BadArgument` callers don't use kw_only, so 0 is fine.
                let pos = field.pos_index.unwrap_or(0);
                quote! { crate::args::ArgErrCtx::BadArgPos { func_name: #func_name, pos: #pos } }
            }
            Some(BadArgStyle::Named) => {
                let arg_name = field.ident.to_string();
                quote! { crate::args::ArgErrCtx::BadArgNamed { func_name: #func_name, arg_name: #arg_name } }
            }
        }
    }

    /// Named (slot-owning) fields in declaration order — everything except
    /// `*args` / `**kwargs`.
    fn named_fields(&self) -> impl Iterator<Item = &Field> {
        self.fields
            .iter()
            .filter(|f| !matches!(f.kind, FieldKind::Varargs | FieldKind::Varkwargs))
    }
}

/// Second parse pass over the fields: resolve implicit kw_only-after-varargs,
/// enforce declaration ordering, assign positional and slot indices, and
/// locate the `*args` / `**kwargs` fields.
fn resolve_field_roles(fields: &mut [Field]) -> syn::Result<(Option<usize>, Option<usize>)> {
    let mut varargs_idx = None;
    let mut varkwargs_idx = None;
    let mut seen_varargs = false;
    let mut seen_varkwargs = false;
    let mut seen_pos_or_kw = false;
    let mut seen_kw_only = false;
    let mut pos_counter: usize = 0;
    let mut slot_counter: usize = 0;
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
                if seen_kw_only {
                    return Err(syn::Error::new(
                        field.ident.span(),
                        "`varargs` cannot appear after keyword-only fields — Python has no \
                         signature form with `*args` following keyword-only parameters",
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
        if !matches!(field.kind, FieldKind::Varargs | FieldKind::Varkwargs) {
            field.slot_index = Some(slot_counter);
            slot_counter += 1;
        }
    }
    Ok((varargs_idx, varkwargs_idx))
}

/// One field's line in the final `Self { ... }` build. `__slots` stays behind
/// the guard, so each slot is drained with `take()` — required ones are
/// guaranteed filled by the conversion phase, and the guard's eventual drop
/// of the emptied struct is a no-op.
fn render_build_field(field: &Field) -> TokenStream {
    let ident = &field.ident;
    match field.kind {
        FieldKind::Varargs => quote! { #ident: __slots.raw.take_varargs(), },
        FieldKind::Varkwargs => quote! { #ident: __slots.raw.take_varkwargs(), },
        _ => match &field.default {
            None => quote! {
                #ident: __slots.#ident.take().expect("required FromArgs slot checked before build"),
            },
            Some(DefaultExpr::DefaultTrait) => quote! {
                #ident: __slots.#ident.take().unwrap_or_default(),
            },
            Some(DefaultExpr::Explicit(expr)) => quote! {
                #ident: __slots.#ident.take().unwrap_or_else(|| { #expr }),
            },
        },
    }
}

impl Field {
    /// The `Param` literal for the runtime spec. `never_matchable` (the
    /// struct's `kwargs_not_supported_yet`) forces `kwarg_id: None`.
    fn render_param(&self, never_matchable: bool) -> TokenStream {
        let name = self.ident.to_string();
        let kwarg_id = if never_matchable {
            quote! { ::std::option::Option::None }
        } else {
            self.kwarg_id_expr()
        };
        let kind = match self.kind {
            FieldKind::PosOnly => quote! { crate::args::ParamKind::PosOnly },
            FieldKind::PosOrKeyword => quote! { crate::args::ParamKind::PosOrKeyword },
            FieldKind::KwOnly => quote! { crate::args::ParamKind::KwOnly },
            FieldKind::Varargs | FieldKind::Varkwargs => unreachable!("varargs/varkwargs own no param slot"),
        };
        let required = self.default.is_none();
        quote! {
            crate::args::Param {
                name: #name,
                kwarg_id: #kwarg_id,
                kind: #kind,
                required: #required,
            }
        }
    }

    /// `Option<StringId>` expression for kwarg matching. Single-char ASCII
    /// field names use the `StringId::from_ascii` fast path (they aren't
    /// `StaticStrings` variants); plain `pos_only` fields without a
    /// `static_string` override get `None` — not matchable by keyword, so a
    /// kwarg with their name falls through to unknown-kwarg handling rather
    /// than the "positional-only passed as keyword" error.
    fn kwarg_id_expr(&self) -> TokenStream {
        let name = self.ident.to_string();
        if matches!(self.kind, FieldKind::PosOnly) && self.static_string.is_none() {
            quote! { ::std::option::Option::None }
        } else if self.static_string.is_none() && name.len() == 1 && name.is_ascii() {
            let byte = name.as_bytes()[0];
            quote! { ::std::option::Option::Some(crate::intern::StringId::from_ascii(#byte)) }
        } else {
            let variant = self.static_string_variant();
            quote! {
                ::std::option::Option::Some(crate::intern::StringId::from_static(
                    crate::intern::StaticStrings::#variant,
                ))
            }
        }
    }

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

/// True for a literal `Vec<Value>` type (by final path segments — the only
/// spellings used in-crate are `Vec<Value>` / `std::vec::Vec<Value>`).
fn is_vec_of_value(ty: &Type) -> bool {
    fn last_segment(ty: &Type) -> Option<&syn::PathSegment> {
        match ty {
            Type::Path(p) => p.path.segments.last(),
            _ => None,
        }
    }
    last_segment(ty).is_some_and(|seg| {
        seg.ident == "Vec"
            && match &seg.arguments {
                syn::PathArguments::AngleBracketed(args) => args.args.iter().any(|arg| match arg {
                    syn::GenericArgument::Type(t) => last_segment(t).is_some_and(|s| s.ident == "Value"),
                    _ => false,
                }),
                _ => false,
            }
    })
}

/// Parsed `#[from_args(...)]` set on the struct itself.
struct StructAttrs {
    name: String,
    style: Style,
    at_most_total: bool,
    kwarg_error_name: Option<String>,
    bad_arg: Option<BadArgStyle>,
    kwargs_not_supported_yet: bool,
}

/// Parse the `#[from_args(...)]` attributes attached to the struct itself.
fn parse_struct_attrs(attrs: &[syn::Attribute]) -> syn::Result<StructAttrs> {
    let mut name: Option<String> = None;
    let mut style: Option<Style> = None;
    let mut at_most_total = false;
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
            } else if meta.path.is_ident("style") {
                if style.is_some() {
                    return Err(meta.error("duplicate `style` attribute"));
                }
                let value: Ident = meta.value()?.parse()?;
                style = Some(match value.to_string().as_str() {
                    "def" => Style::Def,
                    "clinic" => Style::Clinic,
                    "c" => Style::C,
                    "c_named" => Style::CNamed,
                    "unpack" => Style::Unpack,
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!("unknown style `{other}`; expected `def`, `clinic`, `c`, `c_named`, or `unpack`"),
                        ));
                    }
                });
                Ok(())
            } else if meta.path.is_ident("at_most_total") {
                at_most_total = true;
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
            } else if meta.path.is_ident("kwargs_not_supported_yet") {
                kwargs_not_supported_yet = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown struct attribute; expected `name = \"...\"`, `style = def|clinic|c|c_named|unpack`, \
                     `at_most_total`, `kwarg_error_name = \"...\"`, `bad_arg`, `bad_arg_named`, \
                     or `kwargs_not_supported_yet`",
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
    Ok(StructAttrs {
        name,
        style: style.unwrap_or(Style::Clinic),
        at_most_total,
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

// In-module tests are an explicitly approved exception to the "tests live in
// `tests/`" rule: attribute-validation errors never compile into `monty`, so
// `test_cases` cannot reach them, and proc-macro crates cannot expose
// internals to integration tests.
#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use syn::parse_quote;

    use super::expand;

    /// Expand and return the validation error message (panics on success).
    #[track_caller]
    fn expand_err(input: &syn::DeriveInput) -> String {
        expand(input).expect_err("expected a validation error").to_string()
    }

    /// Expand and panic on error — positive control for valid signatures.
    #[track_caller]
    fn expand_ok(input: &syn::DeriveInput) {
        if let Err(err) = expand(input) {
            panic!("expected expansion to succeed, got: {err}");
        }
    }

    #[test]
    fn valid_one_struct_per_style() {
        expand_ok(&parse_quote! {
            #[from_args(name = "search", style = def)]
            struct Def {
                pattern: Value,
                #[from_args(default)]
                flags: Value,
                #[from_args(kw_only, default)]
                extra: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "replace")]
            struct Clinic {
                #[from_args(pos_only)]
                old: Value,
                #[from_args(pos_only)]
                new: Value,
                #[from_args(default = Value::Int(-1))]
                count: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "date", style = c, at_most_total)]
            struct C {
                year: i32,
                month: i32,
                #[from_args(kw_only, default)]
                fold: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "timezone", style = c_named, at_most_total)]
            struct CNamed {
                offset: Value,
                #[from_args(default)]
                name: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "unicodedata.name", style = unpack)]
            struct Unpack {
                #[from_args(pos_only)]
                chr: Value,
                #[from_args(pos_only, default)]
                default: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "print")]
            struct Varargs {
                #[from_args(varargs)]
                objects: Vec<Value>,
                #[from_args(default)]
                sep: Value,
            }
        });
        expand_ok(&parse_quote! {
            #[from_args(name = "dict")]
            struct Varkwargs {
                #[from_args(pos_only, default)]
                iterable: Value,
                #[from_args(varkwargs)]
                kwargs: KwargsValues,
            }
        });
    }

    #[test]
    fn unknown_style() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = fancy)]
            struct S { a: Value }
        });
        assert_snapshot!(err, @"unknown style `fancy`; expected `def`, `clinic`, `c`, `c_named`, or `unpack`");
    }

    #[test]
    fn def_rejects_bad_arg() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = def, bad_arg)]
            struct S { a: Value }
        });
        assert_snapshot!(err, @"`bad_arg`/`bad_arg_named` cannot be combined with `style = def` — CPython `def` binding never type-checks while binding; declare fields as raw `Value` and coerce in the function body");
    }

    #[test]
    fn def_rejects_varargs() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = def)]
            struct S {
                #[from_args(varargs)]
                args: Vec<Value>,
            }
        });
        assert_snapshot!(err, @"`style = def` cannot be combined with `varargs` — a `*args` signature can never raise too-many-positional, so the style has no effect");
    }

    #[test]
    fn unpack_rejects_pos_or_keyword() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = unpack)]
            struct S { a: Value }
        });
        assert_snapshot!(err, @"`style = unpack` models a positional-only `PyArg_UnpackTuple` signature — every positional field must be `pos_only`");
    }

    #[test]
    fn at_most_total_rejects_def_and_unpack() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = def, at_most_total)]
            struct S { a: Value }
        });
        assert_snapshot!(err, @"`at_most_total` cannot be combined with `style = def` or `style = unpack` — the total pre-count models `PyArg_ParseTupleAndKeywords`-family C parsers");
    }

    #[test]
    fn at_most_total_rejects_varargs() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", at_most_total)]
            struct S {
                #[from_args(varargs)]
                args: Vec<Value>,
            }
        });
        assert_snapshot!(err, @"`at_most_total` cannot be combined with `varargs` or `varkwargs` — the up-front total-count check is only meaningful for signatures with a fixed maximum");
    }

    #[test]
    fn kwarg_error_name_requires_def_or_clinic() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", style = c_named, kwarg_error_name = "g")]
            struct S { a: Value }
        });
        assert_snapshot!(err, @"`kwarg_error_name` is only meaningful with `style = def` or the default `clinic` style — the C families defer unknown-kwarg errors past binding and `unpack` callables take no keywords worth renaming");
    }

    #[test]
    fn kwargs_not_supported_yet_rejects_kw_only() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f", kwargs_not_supported_yet)]
            struct S {
                #[from_args(kw_only, default)]
                a: Value,
            }
        });
        assert_snapshot!(err, @"`kwargs_not_supported_yet` cannot be combined with `kw_only` fields — the flag rejects every kwarg up front, so kw_only slots are unreachable");
    }

    #[test]
    fn varargs_must_be_vec_of_value() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(varargs)]
                args: Vec<i64>,
            }
        });
        assert_snapshot!(err, @"`varargs` fields must be `Vec<Value>` — coerce elements in the function body");
    }

    #[test]
    fn required_positional_after_default_rejected() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(default)]
                a: Value,
                b: Value,
            }
        });
        assert_snapshot!(err, @"required positional fields must come before positional fields with defaults — matching Python signatures, and relied on by the runtime binder's fast path");
    }

    #[test]
    fn field_ordering_enforced() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                a: Value,
                #[from_args(pos_only)]
                b: Value,
            }
        });
        assert_snapshot!(err, @"positional-only fields must come before positional-or-keyword, varargs, and keyword-only fields");
    }

    #[test]
    fn varargs_after_kw_only_rejected() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(kw_only, default)]
                a: Value,
                #[from_args(varargs)]
                rest: Vec<Value>,
            }
        });
        assert_snapshot!(err, @"`varargs` cannot appear after keyword-only fields — Python has no signature form with `*args` following keyword-only parameters");
    }

    #[test]
    fn required_kw_only_rejected() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(kw_only)]
                a: Value,
            }
        });
        assert_snapshot!(err, @"keyword-only fields must have a `default` — the runtime binder's fast paths skip the aggregated missing-keyword check, so a required keyword-only parameter would report the wrong error; extend the binder before allowing this");
    }

    #[test]
    fn default_on_varargs_rejected() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(varargs, default)]
                rest: Vec<Value>,
            }
        });
        assert_snapshot!(err, @"`default` and `static_string` cannot be applied to `varargs` / `varkwargs` fields — they configure a named parameter slot, which collector fields don't own");
    }

    #[test]
    fn static_string_on_varkwargs_rejected() {
        let err = expand_err(&parse_quote! {
            #[from_args(name = "f")]
            struct S {
                #[from_args(varkwargs, static_string = "PatternAttr")]
                kwargs: KwargsValues,
            }
        });
        assert_snapshot!(err, @"`default` and `static_string` cannot be applied to `varargs` / `varkwargs` fields — they configure a named parameter slot, which collector fields don't own");
    }

    #[test]
    fn old_flag_spellings_are_rejected() {
        // The pre-`style` boolean flags were removed in one clean break; make
        // sure they fail loudly rather than being silently ignored.
        for old_flag in [
            "py_def",
            "c_error",
            "c_error_named",
            "expected_exact",
            "unpack_tuple",
            "at_most_positional",
        ] {
            let flag = syn::Ident::new(old_flag, proc_macro2::Span::call_site());
            let err = expand_err(&parse_quote! {
                #[from_args(name = "f", #flag)]
                struct S { a: Value }
            });
            assert!(err.starts_with("unknown struct attribute"), "{old_flag}: {err}");
        }
    }
}
