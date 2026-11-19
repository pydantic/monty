//! Parses native signatures with the same Python grammar as interpreted functions.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use ruff_python_ast::{Expr, Number, Operator, Parameter, ParameterWithDefault, Stmt, StmtFunctionDef, UnaryOp};
use ruff_python_parser::parse_module;
use syn::{
    DeriveInput, FnArg, Ident, ItemFn, ItemStruct, LitStr, Pat, Token, Type,
    ext::IdentExt,
    meta::{ParseNestedMeta, parser},
    parse::{Parse, ParseStream, Parser},
    parse_quote,
};

use crate::from_args::{Signature, parse_field_attrs};

pub(super) struct Declaration {
    signature: LitStr,
    bind: Option<Binding>,
    owner: Option<LitStr>,
    kwarg_error_name: Option<LitStr>,
    error_name: Option<LitStr>,
    reject_keywords: bool,
    modifiers: Vec<Ident>,
    converters: Vec<(Ident, Type)>,
}

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut declaration = Self {
            signature: input.parse()?,
            bind: None,
            owner: None,
            kwarg_error_name: None,
            error_name: None,
            reject_keywords: false,
            modifiers: Vec::new(),
            converters: Vec::new(),
        };
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
            parser(|meta| {
                if meta.path.is_ident("bind") {
                    parse_option(&meta, &mut declaration.bind)
                } else if meta.path.is_ident("owner") {
                    parse_option(&meta, &mut declaration.owner)
                } else if meta.path.is_ident("error_name") {
                    parse_option(&meta, &mut declaration.error_name)
                } else if meta.path.is_ident("reject_keywords") {
                    if declaration.reject_keywords {
                        return Err(meta.error("duplicate py_signature option"));
                    }
                    declaration.reject_keywords = true;
                    Ok(())
                } else if meta.path.is_ident("convert") {
                    meta.parse_nested_meta(|field| {
                        let name = field.path.require_ident()?.clone();
                        if declaration.converters.iter().any(|(ident, _)| ident == &name) {
                            return Err(field.error("duplicate converter"));
                        }
                        declaration.converters.push((name, field.value()?.parse()?));
                        Ok(())
                    })
                } else if ["bad_arg", "bad_arg_named", "kwargs_not_supported_yet"]
                    .iter()
                    .any(|option| meta.path.is_ident(option))
                {
                    let ident = meta.path.require_ident()?.clone();
                    if declaration.modifiers.contains(&ident) {
                        return Err(meta.error("duplicate py_signature option"));
                    }
                    declaration.modifiers.push(ident);
                    Ok(())
                } else if meta.path.is_ident("kwarg_error_name") {
                    parse_option(&meta, &mut declaration.kwarg_error_name)
                } else {
                    Err(meta.error("unknown py_signature option"))
                }
            })
            .parse2(input.parse()?)?;
        }
        Ok(declaration)
    }
}

fn parse_option<T: Parse>(meta: &ParseNestedMeta<'_>, slot: &mut Option<T>) -> syn::Result<()> {
    if slot.is_some() {
        return Err(meta.error("duplicate py_signature option"));
    }
    *slot = Some(meta.value()?.parse()?);
    Ok(())
}

#[derive(Clone, Copy, Default)]
enum Binding {
    #[default]
    Shared,
    Mutable,
    Owned,
}

impl Parse for Binding {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident = input.call(Ident::parse_any)?;
        match ident.to_string().as_str() {
            "shared" => Ok(Self::Shared),
            "mut" => Ok(Self::Mutable),
            "owned" => Ok(Self::Owned),
            _ => Err(syn::Error::new(ident.span(), "expected shared, mut or owned")),
        }
    }
}

impl Binding {
    fn argument_type(self, ty: &Type) -> syn::Result<&Type> {
        match (self, ty) {
            (Self::Shared, Type::Reference(r)) if r.mutability.is_none() => Ok(&r.elem),
            (Self::Mutable, Type::Reference(r)) if r.mutability.is_some() => Ok(&r.elem),
            (Self::Owned, ty) if !matches!(ty, Type::Reference(_)) => Ok(ty),
            _ => Err(syn::Error::new_spanned(
                ty,
                match self {
                    Self::Shared => "shared binding requires &Args",
                    Self::Mutable => "bind = mut requires &mut Args",
                    Self::Owned => "bind = owned requires Args by value",
                },
            )),
        }
    }

    fn guard(self) -> TokenStream {
        match self {
            Self::Shared => quote!(crate::defer_drop!(__bound, vm);),
            Self::Mutable => quote!(crate::defer_drop_mut!(__bound, vm);),
            Self::Owned => quote!(),
        }
    }
}

pub(super) fn expand(declaration: &Declaration, mut function: ItemFn) -> syn::Result<TokenStream> {
    let def = parse_function(declaration)?;
    let name = def.name.as_str();
    let error = |message: &str| syn::Error::new(declaration.signature.span(), message);
    let binding = declaration.bind.unwrap_or_default();
    if let Some(adapter) = positional_iterator(declaration, &def, &mut function)? {
        return Ok(adapter);
    }
    if let Some(param) = infer_single_argument(declaration, &def, &function) {
        if declaration.bind.is_some() {
            return Err(error("bind requires an argument struct or iterator"));
        }
        return single_positional_adapter(function, declaration, param, &callable_name(declaration, name));
    }
    if let Some(adapter) = two_positional_adapter(declaration, &def, &mut function)? {
        return Ok(adapter);
    }
    let bound = function.sig.inputs.iter_mut().find_map(|input| {
        let FnArg::Typed(arg) = input else { return None };
        matches!(arg.pat.as_ref(), Pat::Ident(p) if p.ident == "args").then_some(arg)
    });
    let Some(bound) = bound else {
        if let Some(name) = zero_arg_name(declaration, &def)
            && function.sig.inputs.iter().any(
                |arg| matches!(arg, FnArg::Typed(arg) if matches!(arg.pat.as_ref(), Pat::Ident(p) if p.ident == "vm")),
            )
        {
            if declaration.bind.is_some() {
                return Err(error("bind requires an argument struct or iterator"));
            }
            // Validate the signature's options before choosing the no-argument adapter.
            arguments(declaration, &def, None)?;
            function
                .sig
                .inputs
                .push(parse_quote!(__py_args: crate::args::ArgValues));
            function
                .block
                .stmts
                .insert(0, parse_quote!(__py_args.check_zero_args(#name, vm.heap)?;));
            return Ok(quote!(#function));
        }
        return Err(error("expected a parameter named args with the bound argument type"));
    };
    let Pat::Ident(pat) = &*bound.pat else {
        unreachable!("checked the argument pattern")
    };
    if pat.by_ref.is_some() || pat.subpat.is_some() {
        return Err(error("expected a name for the bound arguments"));
    }
    let ty = &bound.ty;
    let bound_name = match binding.argument_type(ty)? {
        Type::Path(path) => path.path.get_ident(),
        _ => None,
    }
    .cloned()
    .ok_or_else(|| syn::Error::new_spanned(ty, "expected the name of the bound argument type"))?;
    if matches!(binding, Binding::Owned) {
        validate_owned_arguments(&function.block, &bound_name, &pat.ident)?;
    }
    let (bound_struct, implementation) = arguments(declaration, &def, Some(&bound_name))?;
    let guard = binding.guard();
    let body = function.block.stmts;
    function.block = Box::new(parse_quote!({
        #bound_struct
        #implementation
        let __bound = #bound_name::from_args(__py_args, vm)?;
        #guard
        let #pat: #ty = __bound;
        #(#body)*
    }));
    bound.pat = parse_quote!(__py_args);
    bound.ty = parse_quote!(crate::args::ArgValues);
    Ok(quote!(#function))
}

/// An owned binder has no guard. Require each field to move into a named binding.
fn validate_owned_arguments(body: &syn::Block, ty: &Ident, args: &Ident) -> syn::Result<()> {
    let binding = owned_argument_pattern(body, args)
        .ok_or_else(|| syn::Error::new(args.span(), "destructure the owned arguments at the start of the body"))?;
    if !binding.path.is_ident(ty) {
        return Err(syn::Error::new_spanned(
            &binding.path,
            "expected the destructured argument type",
        ));
    }
    if let Some(rest) = &binding.rest {
        return Err(syn::Error::new_spanned(
            rest,
            "bind every owned argument or borrow the argument struct",
        ));
    }
    for field in &binding.fields {
        if !matches!(&*field.pat, Pat::Ident(ident) if ident.by_ref.is_none() && ident.subpat.is_none()) {
            return Err(syn::Error::new_spanned(&field.pat, "bind each owned argument by name"));
        }
    }
    Ok(())
}

fn owned_argument_pattern<'a>(body: &'a syn::Block, name: &Ident) -> Option<&'a syn::PatStruct> {
    let syn::Stmt::Local(local) = body.stmts.first()? else {
        return None;
    };
    let Pat::Struct(pattern) = &local.pat else { return None };
    let init = local.init.as_ref()?;
    let syn::Expr::Path(source) = &*init.expr else {
        return None;
    };
    (local.attrs.is_empty() && init.diverge.is_none() && source.path.is_ident(name)).then_some(pattern)
}

/// A named binder lets dispatchers bind after method lookup or transfer owned fields to the VM.
pub(super) fn expand_struct(declaration: &Declaration, item: ItemStruct) -> syn::Result<TokenStream> {
    if declaration.bind.is_some() {
        return Err(syn::Error::new(
            declaration.signature.span(),
            "bind is only used on handlers",
        ));
    }
    if !matches!(item.fields, syn::Fields::Unit)
        || !item.generics.params.is_empty()
        || item.generics.where_clause.is_some()
    {
        return Err(syn::Error::new_spanned(item, "expected a unit struct without generics"));
    }
    let (mut bound, implementation) = arguments(declaration, &parse_function(declaration)?, Some(&item.ident))?;
    bound.attrs = item.attrs;
    bound.vis = item.vis;
    if let syn::Data::Struct(data) = &mut bound.data {
        for field in &mut data.fields {
            field.vis = bound.vis.clone();
        }
    }
    Ok(quote!(#bound #implementation))
}

fn parse_function(declaration: &Declaration) -> syn::Result<StmtFunctionDef> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let signature = literal.value();
    let source = if signature.starts_with('(') {
        let name = declaration
            .error_name
            .as_ref()
            .ok_or_else(|| error("a parameter list requires error_name"))?;
        format!("def {}{signature}: pass", name.value())
    } else {
        format!("def {signature}: pass")
    };
    let parsed = parse_module(&source).map_err(|e| error(&e.to_string()))?;
    let mut body = parsed.into_syntax().body.into_iter();
    let (Some(Stmt::FunctionDef(def)), None) = (body.next(), body.next()) else {
        return Err(error("expected one Python function signature"));
    };
    if !matches!(&def.body[..], [Stmt::Pass(_)]) {
        return Err(error("expected a Python function signature without a body"));
    }
    Ok(def)
}

/// The raw helpers return owned Python values and represent an omitted argument with `None`.
fn raw_positional(declaration: &Declaration, def: &StmtFunctionDef) -> bool {
    let params = &def.parameters;
    declaration.modifiers.is_empty()
        && declaration.converters.is_empty()
        && declaration.kwarg_error_name.is_none()
        && params.args.is_empty()
        && params.kwonlyargs.is_empty()
        && params.kwarg.is_none()
        && params.iter().all(|p| p.annotation().is_none())
        && params
            .posonlyargs
            .iter()
            .all(|p| matches!(p.default(), None | Some(Expr::EllipsisLiteral(_))))
}

/// Positional iterators retain the inline storage when a handler needs the whole slice.
fn positional_iterator(
    declaration: &Declaration,
    def: &StmtFunctionDef,
    function: &mut ItemFn,
) -> syn::Result<Option<TokenStream>> {
    let Some(arg) = function.sig.inputs.iter_mut().find_map(|input| {
        let FnArg::Typed(arg) = input else { return None };
        let ty = match arg.ty.as_ref() {
            Type::Reference(r) => r.elem.as_ref(),
            ty => ty,
        };
        matches!(ty, Type::Path(p) if p.path.is_ident("ArgPosIter")).then_some(arg)
    }) else {
        return Ok(None);
    };
    let params = &def.parameters;
    let receiver = params.posonlyargs.first().is_some_and(|p| p.name().as_str() == "self");
    let positional = &params.posonlyargs[usize::from(receiver)..];
    if !raw_positional(declaration, def)
        || declaration.reject_keywords
        || (declaration.owner.is_some() && (!receiver || declaration.error_name.is_some()))
        || (receiver && params.posonlyargs[0].default().is_some())
        || (params.vararg.is_some() && !positional.is_empty())
    {
        return Err(syn::Error::new(
            declaration.signature.span(),
            "an argument iterator requires raw positional-only parameters or a single *args",
        ));
    }
    let name = callable_name(declaration, def.name.as_str());
    let bind = if params.vararg.is_some() {
        quote!(__py_args.into_pos_only(#name, vm.heap)?)
    } else {
        let min = positional.iter().take_while(|p| p.default().is_none()).count();
        let max = positional.len();
        quote!(__py_args.bind_positional_range(#name, #min, #max, vm.heap)?)
    };
    let binding = &arg.pat;
    let ty = &arg.ty;
    let mode = declaration.bind.unwrap_or_default();
    mode.argument_type(ty)?;
    let guard = mode.guard();
    let body = &function.block;
    *function.block = parse_quote!({
        let __bound = #bind;
        #guard
        let #binding: #ty = __bound;
        #body
    });
    arg.ty = parse_quote!(crate::args::ArgValues);
    arg.pat = parse_quote!(__py_args);
    Ok(Some(quote!(#function)))
}

fn arguments(
    declaration: &Declaration,
    def: &StmtFunctionDef,
    named: Option<&Ident>,
) -> syn::Result<(DeriveInput, TokenStream)> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let name = def.name.as_str();
    let params = &def.parameters;
    for (field, _) in &declaration.converters {
        let name = field.to_string();
        if !params
            .posonlyargs
            .iter()
            .chain(&params.args)
            .chain(&params.kwonlyargs)
            .any(|param| param.name().as_str() == name)
            || name == "self"
        {
            return Err(syn::Error::new(field.span(), "unknown converter parameter"));
        }
    }
    let receiver = params.posonlyargs.first().filter(|p| p.name().as_str() == "self");
    if receiver.is_some_and(|p| p.annotation().is_some() || p.default().is_some()) {
        return Err(error("a bound receiver cannot have an annotation or a default"));
    }
    let receiver = receiver.is_some();
    let mut fields = Vec::new();
    for param in params.posonlyargs.iter().skip(usize::from(receiver)) {
        fields.push(named_field(param, &quote!(pos_only,), declaration)?);
    }
    for param in &params.args {
        fields.push(named_field(param, &quote!(), declaration)?);
    }
    if let Some(rest) = &params.vararg {
        let ident = field_name(rest, literal)?;
        if rest.annotation.is_some() {
            return Err(error("typed variadic parameters are unsupported"));
        }
        fields.push(quote!(#[from_args(varargs)] #ident: Vec<crate::value::Value>));
    }
    for param in &params.kwonlyargs {
        fields.push(named_field(param, &quote!(kw_only,), declaration)?);
    }
    if let Some(kwargs) = &params.kwarg {
        let ident = field_name(kwargs, literal)?;
        if kwargs.annotation.is_some() {
            return Err(error("typed variadic parameters are unsupported"));
        }
        fields.push(quote!(#[from_args(varkwargs)] #ident: crate::args::KwargsValues));
    }

    let bound_name = named
        .cloned()
        .unwrap_or_else(|| format_ident!("{}Args", snake_to_pascal(name)));
    if declaration.owner.is_some() && declaration.error_name.is_some() {
        return Err(error("choose either owner or error_name"));
    }
    if declaration.owner.is_some() && !receiver {
        return Err(error("owner requires a positional-only self parameter"));
    }
    let qualified_name = callable_name(declaration, name);
    let modifiers = &declaration.modifiers;
    let kwarg_error = declaration
        .kwarg_error_name
        .as_ref()
        .map(|name| quote!(, kwarg_error_name = #name));
    let input: DeriveInput = syn::parse2(quote! {
        #[from_args(name = #qualified_name #kwarg_error #(, #modifiers)*)]
        struct #bound_name { #(#fields,)* }
    })?;
    let signature = Signature::parse(&input)?;
    let drop_args = signature.drop_bound_args();
    let from_args = if declaration.reject_keywords {
        let invalid = || error("reject_keywords requires a fixed raw positional signature");
        if !raw_positional(declaration, def) || params.vararg.is_some() {
            return Err(invalid());
        }
        let positional = &params.posonlyargs[usize::from(receiver)..];
        let required = positional.iter().take_while(|p| p.default().is_none()).count();
        let helper = match (required, positional.len()) {
            (0, 0) => quote!(check_zero_args),
            (0, 1) => quote!(get_zero_one_arg),
            (1, 1) => quote!(get_one_arg),
            (1, 2) => quote!(get_one_two_args),
            (2, 2) => quote!(get_two_args),
            _ => return Err(invalid()),
        };
        let names = positional
            .iter()
            .map(|p| field_name(&p.parameter, literal))
            .collect::<syn::Result<Vec<_>>>()?;
        let binding = if let [one] = names.as_slice() {
            quote!(#one)
        } else {
            quote!((#(#names),*))
        };
        quote! {
            impl #bound_name {
                pub(crate) fn from_args(args: crate::args::ArgValues, vm: &mut crate::bytecode::VM<'_>)
                    -> crate::exception_private::RunResult<Self> {
                    let args = args.reject_kwargs(#qualified_name, vm.heap)?;
                    let #binding = args.#helper(#qualified_name, vm.heap)?;
                    Ok(Self { #(#names),* })
                }
            }
        }
    } else {
        signature.render(receiver)
    };
    let mut bound_struct = input;
    bound_struct.attrs.clear();
    if let syn::Data::Struct(data) = &mut bound_struct.data {
        for field in &mut data.fields {
            field.attrs.clear();
        }
    }

    Ok((bound_struct, quote!(#from_args #drop_args)))
}

fn zero_arg_name(declaration: &Declaration, def: &StmtFunctionDef) -> Option<String> {
    if !declaration.modifiers.is_empty() || declaration.kwarg_error_name.is_some() || declaration.reject_keywords {
        return None;
    }
    let params = &def.parameters;
    let receiver_only =
        params.iter().count() == 1 && params.posonlyargs.first().is_some_and(|p| p.name().as_str() == "self");
    (params.is_empty() || receiver_only).then(|| callable_name(declaration, def.name.as_str()))
}

fn callable_name(declaration: &Declaration, name: &str) -> String {
    if let Some(owner) = &declaration.owner {
        format!("{}.{name}", owner.value())
    } else {
        declaration
            .error_name
            .as_ref()
            .map_or_else(|| name.to_owned(), LitStr::value)
    }
}

fn single_required_positional(def: &StmtFunctionDef) -> Option<&ParameterWithDefault> {
    let params = &def.parameters;
    let [param] = params.posonlyargs.as_slice() else {
        return None;
    };
    (params.iter().count() == 1 && param.default().is_none()).then_some(param)
}

/// A Rust parameter matching the Python name opts into direct binding. A named args
/// struct still uses the general binder for the same Python signature.
fn infer_single_argument<'a>(
    declaration: &Declaration,
    def: &'a StmtFunctionDef,
    function: &ItemFn,
) -> Option<&'a ParameterWithDefault> {
    if declaration.reject_keywords
        || declaration.owner.is_some()
        || declaration.kwarg_error_name.is_some()
        || !declaration.modifiers.is_empty()
    {
        return None;
    }
    let param = single_required_positional(def)?;
    let mut inputs =
        function.sig.inputs.iter().filter(
            |input| !matches!(input, FnArg::Typed(arg) if matches!(&*arg.pat, Pat::Ident(p) if p.ident == "vm")),
        );
    let (Some(FnArg::Typed(arg)), None) = (inputs.next(), inputs.next()) else {
        return None;
    };
    let name = param.name().as_str();
    (matches!(&*arg.pat, Pat::Ident(p) if p.ident == name)
        && name != "self"
        && (name != "args" || *arg.ty == parse_quote!(&Value)))
    .then_some(param)
}

/// Bind a pair of required positional-only parameters directly from inline storage.
fn two_positional_adapter(
    declaration: &Declaration,
    def: &StmtFunctionDef,
    function: &mut ItemFn,
) -> syn::Result<Option<TokenStream>> {
    let pair: Type = parse_quote!((&Value, &Value));
    let Some(arg) = function.sig.inputs.iter_mut().find_map(|input| {
        let FnArg::Typed(arg) = input else { return None };
        (*arg.ty == pair).then_some(arg)
    }) else {
        return Ok(None);
    };
    let literal = &declaration.signature;
    let error = |message| syn::Error::new(literal.span(), message);
    let params = &def.parameters;
    let [first, second] = params.posonlyargs.as_slice() else {
        return Err(error("a direct pair requires two positional-only parameters"));
    };
    if !raw_positional(declaration, def)
        || declaration.reject_keywords
        || params.vararg.is_some()
        || first.default().is_some()
        || second.default().is_some()
        || first.name().as_str() == "self"
        || declaration.bind.is_some()
        || declaration.owner.is_some()
    {
        return Err(error("a direct pair requires two raw positional-only arguments"));
    }
    let first = field_name(&first.parameter, literal)?;
    let second = field_name(&second.parameter, literal)?;
    let ordered: Pat = parse_quote!((#first, #second));
    let named =
        matches!(arg.pat.as_ref(), Pat::Ident(p) if p.ident == "args" && p.by_ref.is_none() && p.subpat.is_none());
    if *arg.pat != ordered && !named {
        return Err(error("expected args or a pair in signature order"));
    }
    let binding = &arg.pat;
    let name = callable_name(declaration, def.name.as_str());
    let body = &function.block;
    *function.block = parse_quote!({
        let (__first, __second) = __py_args.get_two_positional_args(#name, vm.heap)?;
        crate::defer_drop!(__first, vm);
        crate::defer_drop!(__second, vm);
        let #binding = (__first, __second);
        #body
    });
    arg.pat = parse_quote!(__py_args);
    arg.ty = parse_quote!(crate::args::ArgValues);
    Ok(Some(quote!(#function)))
}

/// Bind the object with the same arity check and drop guard as a handwritten one-argument builtin.
fn single_positional_adapter(
    mut function: ItemFn,
    declaration: &Declaration,
    param: &ParameterWithDefault,
    name: &str,
) -> syn::Result<TokenStream> {
    let field = field_name(&param.parameter, &declaration.signature)?;
    let arg = function
        .sig
        .inputs
        .iter_mut()
        .find_map(|input| {
            let FnArg::Typed(arg) = input else { return None };
            matches!(arg.pat.as_ref(), Pat::Ident(pattern) if pattern.ident == field).then_some(arg)
        })
        .ok_or_else(|| {
            syn::Error::new(
                field.span(),
                format!("expected a parameter named `{field}` of type &Value"),
            )
        })?;
    if let Some((unknown, _)) = declaration.converters.iter().find(|(name, _)| name != &field) {
        return Err(syn::Error::new(unknown.span(), "unknown converter parameter"));
    }
    let custom = !declaration.converters.is_empty();
    let bind = if custom || param.annotation().is_some() {
        let typed = syn::Field::parse_named.parse2(named_field(param, &quote!(pos_only,), declaration)?)?;
        let expected = &arg.ty;
        let ty = &typed.ty;
        let convert = if let Some(extract) = parse_field_attrs(&typed.attrs)?.extract_with {
            quote!(#extract(#field, vm, crate::args::ArgErrCtx::Plain)?)
        } else {
            quote!(<#ty as crate::args::FromValue>::extract(#field, vm, crate::args::ArgErrCtx::Plain)?)
        };
        let scalar = matches!(param.annotation(), Some(Expr::Name(name)) if matches!(name.id.as_str(), "bool" | "c_int" | "i64" | "float"));
        let guard = match &**expected {
            Type::Reference(reference) if reference.mutability.is_none() => quote!(crate::defer_drop!(#field, vm);),
            _ if scalar && !custom => quote!(),
            _ => {
                return Err(syn::Error::new_spanned(
                    expected,
                    "borrow the converted argument so the binder can release its heap references",
                ));
            }
        };
        quote!(let #field: #ty = #convert; #guard let #field: #expected = #field;)
    } else {
        if !matches!(arg.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_none()
            && matches!(reference.elem.as_ref(), Type::Path(path) if path.path.is_ident("Value")))
        {
            return Err(syn::Error::new_spanned(
                &arg.ty,
                "expected &Value for a single positional argument",
            ));
        }
        quote!(crate::defer_drop!(#field, vm);)
    };
    *arg.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({
        let #field = #field.get_one_arg(#name, vm.heap)?;
        #bind
        #body
    }));
    Ok(quote!(#function))
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

fn field_name(param: &Parameter, literal: &LitStr) -> syn::Result<syn::Ident> {
    syn::parse_str(param.name.as_str())
        .map_err(|_| syn::Error::new(literal.span(), "parameter name must also be a Rust identifier"))
}

fn named_field(
    param: &ParameterWithDefault,
    role: &TokenStream,
    declaration: &Declaration,
) -> syn::Result<TokenStream> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let ident = field_name(&param.parameter, literal)?;
    let converter = declaration
        .converters
        .iter()
        .find(|(field, _)| field == &ident)
        .map(|(_, ty)| ty);
    if let Some(ty) = converter {
        let (ty, default) = match param.default() {
            None => (quote!(#ty), quote!()),
            Some(Expr::EllipsisLiteral(_)) => (quote!(Option<#ty>), quote!(default)),
            Some(expr) if let Some(n) = integer_default(expr) => (quote!(#ty), quote!(default = <#ty>::from(#n))),
            _ => return Err(error("unsupported custom converter default")),
        };
        return Ok(quote!(#[from_args(#role #default)] #ident: #ty));
    }
    let annotation = match param.annotation() {
        None => None,
        Some(Expr::Name(name)) => Some(name.id.as_str()),
        Some(Expr::BinOp(union))
            if union.op == Operator::BitOr
                && matches!(
                    (&*union.left, &*union.right),
                    (Expr::Name(name), Expr::NoneLiteral(_)) | (Expr::NoneLiteral(_), Expr::Name(name)) if name.id.as_str() == "str"
                ) =>
        {
            Some("str | None")
        }
        _ => return Err(error("unsupported native parameter annotation")),
    };
    let ty = match annotation {
        None if matches!(param.default(), Some(Expr::EllipsisLiteral(_))) => quote!(Option<crate::value::Value>),
        None if matches!(param.default(), Some(Expr::StringLiteral(_))) => quote!(crate::args::DefaultedStringArg),
        None => quote!(crate::value::Value),

        Some("c_int") => quote!(i32),
        Some("bool") => quote!(crate::args::LaxBool),
        Some("str") => quote!(crate::args::StrArg),
        Some("str | None") => quote!(Option<crate::args::StrArg>),
        Some("float") => quote!(f64),
        Some("i64") => quote!(i64),
        Some("int" | "SupportsIndex") => quote!(crate::args::IntArg),
        Some(_) => return Err(error("unsupported native parameter annotation")),
    };
    let ty = if annotation.is_some() && matches!(param.default(), Some(Expr::EllipsisLiteral(_))) {
        quote!(Option<#ty>)
    } else {
        ty
    };
    let default = match (annotation, param.default()) {
        (_, None) => quote!(),
        (None | Some("str" | "int" | "c_int" | "SupportsIndex" | "i64" | "bool"), Some(Expr::EllipsisLiteral(_))) => {
            quote!(default)
        }
        (Some("str | None"), Some(Expr::NoneLiteral(_))) => quote!(default),
        (None, Some(Expr::NoneLiteral(_))) => quote!(default = crate::value::Value::None),
        (Some("bool"), Some(Expr::BooleanLiteral(b))) => {
            let value = b.value;
            quote!(default = crate::args::LaxBool::new(#value))
        }
        (Some("c_int"), Some(expr)) => {
            let value = integer_default(expr)
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| error("expected a C int literal"))?;
            quote!(default = #value)
        }
        (None, Some(expr)) if let Some(value) = integer_default(expr) => {
            quote!(default = crate::value::Value::Int(#value))
        }
        (None, Some(Expr::BooleanLiteral(b))) => {
            let value = b.value;
            quote!(default = crate::value::Value::Bool(#value))
        }
        (None | Some("float"), Some(Expr::NumberLiteral(n)))
            if let Number::Float(value) = n.value
                && value.is_finite() =>
        {
            if annotation.is_some() {
                quote!(default = #value)
            } else {
                quote!(default = crate::value::Value::Float(#value))
            }
        }
        (None, Some(Expr::StringLiteral(s))) => {
            let value = s.value.to_str();
            quote!(default = crate::args::DefaultedStringArg::Omitted(#value))
        }
        (Some("int" | "SupportsIndex"), Some(expr)) => {
            let integer = full_width_default(expr).ok_or_else(|| error("expected an integer literal"))?;
            quote!(default = #integer)
        }
        (Some("i64"), Some(expr)) => {
            let n = integer_default(expr).ok_or_else(|| error("expected a signed 64-bit integer literal"))?;
            quote!(default = #n)
        }
        _ => return Err(error("unsupported default for this parameter type")),
    };
    let extractor = match annotation {
        Some("SupportsIndex") => quote!(extract_with = crate::args::extract_index,),
        Some("i64") => quote!(extract_with = crate::args::extract_i64_index,),
        Some("str | None") => quote!(extract_with = crate::args::extract_nullable_str,),
        _ => quote!(),
    };
    Ok(quote!(#[from_args(#role #extractor #default)] #ident: #ty))
}

fn full_width_default(expr: &Expr) -> Option<TokenStream> {
    if let Some(n) = integer_default(expr) {
        return Some(quote!(crate::args::IntArg::from(#n)));
    }
    let (integer, negative) = integer_literal(expr)?;
    let literal = syn::parse_str::<syn::LitInt>(&integer.to_string().to_ascii_lowercase()).ok()?;
    let sign = if negative { "-" } else { "" };
    let decimal = format!("{sign}{}", literal.base10_digits());
    Some(quote!({
        let integer = crate::types::LongInt::new(#decimal.parse().expect("validated integer literal"));
        crate::args::IntArg::from_index(integer.into_value(vm.heap), vm)
    }))
}

fn integer_default(expr: &Expr) -> Option<i64> {
    let (integer, negative) = integer_literal(expr)?;
    let magnitude = i128::from(integer.as_u64()?);
    i64::try_from(if negative { -magnitude } else { magnitude }).ok()
}

fn integer_literal(mut expr: &Expr) -> Option<(&ruff_python_ast::Int, bool)> {
    let mut negative = false;
    while let Expr::UnaryOp(op) = expr {
        if op.op != UnaryOp::USub {
            return None;
        }
        negative = !negative;
        expr = &op.operand;
    }
    let Expr::NumberLiteral(number) = expr else { return None };
    let Number::Int(integer) = &number.value else {
        return None;
    };
    Some((integer, negative))
}

#[cfg(test)]
#[path = "../tests/unit/py_signature.rs"]
mod tests;
