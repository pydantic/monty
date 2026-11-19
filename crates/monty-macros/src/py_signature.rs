//! Parses native signatures with the same Python grammar as interpreted functions.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use ruff_python_ast::{Expr, Number, Operator, Parameter, ParameterWithDefault, Stmt, StmtFunctionDef, UnaryOp};
use ruff_python_parser::parse_module;
use syn::{
    DeriveInput, FnArg, Ident, ItemFn, ItemStruct, LitStr, Pat, Token, Type,
    meta::{ParseNestedMeta, parser},
    parse::{Parse, ParseStream, Parser},
    parse_quote,
};

use crate::from_args::Signature;

pub(super) struct Declaration {
    signature: LitStr,
    style: Option<Ident>,
    owner: Option<LitStr>,
    kwarg_error_name: Option<LitStr>,
    error_name: Option<LitStr>,
    modifiers: Vec<Ident>,
    converters: Vec<(Ident, Type)>,
}

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut declaration = Self {
            signature: input.parse()?,
            style: None,
            owner: None,
            kwarg_error_name: None,
            error_name: None,
            modifiers: Vec::new(),
            converters: Vec::new(),
        };
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
            parser(|meta| {
                if meta.path.is_ident("style") {
                    parse_option(&meta, &mut declaration.style)
                } else if meta.path.is_ident("owner") {
                    parse_option(&meta, &mut declaration.owner)
                } else if meta.path.is_ident("error_name") {
                    parse_option(&meta, &mut declaration.error_name)
                } else if meta.path.is_ident("convert") {
                    meta.parse_nested_meta(|field| {
                        let name = field.path.require_ident()?.clone();
                        if declaration.converters.iter().any(|(ident, _)| ident == &name) {
                            return Err(field.error("duplicate converter"));
                        }
                        declaration.converters.push((name, field.value()?.parse()?));
                        Ok(())
                    })
                } else if [
                    "at_most_total",
                    "bad_arg",
                    "bad_arg_named",
                    "vectorcall",
                    "kwargs_not_supported_yet",
                ]
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

pub(super) fn expand(declaration: &Declaration, mut function: ItemFn) -> syn::Result<TokenStream> {
    let def = parse_function(declaration)?;
    if declaration
        .style
        .as_ref()
        .is_some_and(|s| s == "one_arg" || s == "two_args")
    {
        return positional_adapter(declaration, &def, function);
    }
    for input in &mut function.sig.inputs {
        let FnArg::Typed(arg) = input else { continue };
        let Pat::Struct(pattern) = arg.pat.as_ref() else {
            continue;
        };
        let name = pattern.path.require_ident()?;
        let (ty, borrowed) = match arg.ty.as_ref() {
            Type::Reference(reference) if reference.mutability.is_none() => (reference.elem.as_ref(), true),
            ty => (ty, false),
        };
        if !matches!(ty, Type::Path(path) if path.path.is_ident(name)) {
            return Err(syn::Error::new_spanned(ty, "expected the destructured argument type"));
        }
        let (bound, implementation) = arguments(declaration, &def, Some(name))?;
        let guard = borrowed.then(|| quote!(crate::defer_drop!(__bound, vm);));
        let original = function.block;
        function.block = Box::new(parse_quote!({
            let __bound = #name::from_args(__py_args, vm)?;
            #guard
            let #pattern = __bound;
            #original
        }));
        arg.pat = parse_quote!(__py_args);
        arg.ty = parse_quote!(crate::args::ArgValues);
        return Ok(quote!(#bound #implementation #function));
    }
    let (bound_struct, implementation) = arguments(declaration, &def, None)?;
    let bound_name = &bound_struct.ident;
    let error = |message: &str| syn::Error::new(declaration.signature.span(), message);
    let bound = function.sig.inputs.iter_mut().find_map(|input| {
        let FnArg::Typed(arg) = input else { return None };
        let Type::Reference(reference) = arg.ty.as_ref() else {
            return None;
        };
        let Type::Path(path) = reference.elem.as_ref() else {
            return None;
        };
        path.path.is_ident(bound_name).then_some(arg)
    });
    let Some(bound) = bound else {
        if let Some(name) = zero_arg_name(declaration, &def)
            && function.sig.inputs.iter().any(
                |arg| matches!(arg, FnArg::Typed(arg) if matches!(arg.pat.as_ref(), Pat::Ident(p) if p.ident == "vm")),
            )
        {
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
        return Err(error(&format!("expected a parameter of type &{bound_name}")));
    };
    let Pat::Ident(pattern) = &*bound.pat else {
        return Err(error("expected a name for the bound arguments"));
    };
    let arg = pattern.ident.clone();
    let bound_ty = bound.ty.clone();
    let guard = if matches!(&*bound_ty, Type::Reference(reference) if reference.mutability.is_some()) {
        quote!(crate::defer_drop_mut!)
    } else {
        quote!(crate::defer_drop!)
    };
    *bound.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({
        let #arg = #bound_name::from_args(#arg, vm)?;
        #guard(#arg, vm);
        #body
    }));
    Ok(quote! {
        #bound_struct
        #implementation
        #function
    })
}

/// A named binder lets dispatchers bind after method lookup or transfer owned fields to the VM.
pub(super) fn expand_struct(declaration: &Declaration, item: ItemStruct) -> syn::Result<TokenStream> {
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

fn arguments(
    declaration: &Declaration,
    def: &StmtFunctionDef,
    named: Option<&Ident>,
) -> syn::Result<(DeriveInput, TokenStream)> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let name = def.name.as_str();
    let params = &def.parameters;
    let is_def = declaration.style.as_ref().is_some_and(|style| style == "def");
    if is_def && (!declaration.converters.is_empty() || params.iter().any(|param| param.annotation().is_some())) {
        return Err(error(
            "leave parameters unannotated with style = def and convert values in the function body",
        ));
    }
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
    let receiver = params.posonlyargs.first().is_some_and(|p| p.name().as_str() == "self");
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
    let style = declaration.style.as_ref().map(|style| quote!(, style = #style));
    let bad_arg = (receiver
        && !is_def
        && !declaration
            .modifiers
            .iter()
            .any(|m| m == "bad_arg" || m == "bad_arg_named"))
    .then(|| quote!(, bad_arg));
    let kwarg_error = declaration
        .kwarg_error_name
        .as_ref()
        .map(|name| quote!(, kwarg_error_name = #name));
    let input: DeriveInput = syn::parse2(quote! {
        #[from_args(name = #qualified_name #style #bad_arg #kwarg_error #(, #modifiers)*)]
        struct #bound_name { #(#fields,)* }
    })?;
    let signature = Signature::parse(&input)?;
    let from_args = if let Some(name) = zero_arg_name(declaration, def) {
        quote! {
            impl #bound_name {
                pub(crate) fn from_args(args: crate::args::ArgValues, vm: &mut crate::bytecode::VM<'_>)
                    -> crate::exception_private::RunResult<Self> {
                    args.check_zero_args(#name, vm.heap)?;
                    Ok(Self {})
                }
            }
        }
    } else {
        signature.render(receiver)
    };
    let drop_args = signature.drop_bound_args();
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
    if declaration.style.is_some() || !declaration.modifiers.is_empty() || declaration.kwarg_error_name.is_some() {
        return None;
    }
    let params = &def.parameters;
    let receiver_only = params.iter().count() == 1
        && params
            .posonlyargs
            .first()
            .is_some_and(|p| p.name().as_str() == "self" && p.annotation().is_none() && p.default().is_none());
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

fn positional_adapter(
    declaration: &Declaration,
    def: &StmtFunctionDef,
    mut function: ItemFn,
) -> syn::Result<TokenStream> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    if !declaration.modifiers.is_empty() || !declaration.converters.is_empty() || declaration.error_name.is_some() {
        return Err(error(
            "fixed-arity helpers do not accept parser modifiers or custom converters",
        ));
    }
    let name = def.name.as_str();
    let params = &def.parameters;
    if declaration.style.as_ref().is_some_and(|style| style == "one_arg") {
        let Some(param) = params
            .posonlyargs
            .first()
            .filter(|p| params.iter().count() == 1 && p.default().is_none() && p.annotation().is_none())
        else {
            return Err(error(
                "`style = one_arg` requires one unannotated positional-only parameter without a default",
            ));
        };
        if declaration.owner.is_some() || declaration.kwarg_error_name.is_some() {
            return Err(error("`style = one_arg` uses the callable name for errors"));
        }
        let field = field_name(&param.parameter, literal)?;
        return single_positional_adapter(function, name, &field);
    }
    let [first, second] = params.posonlyargs.as_slice() else {
        return Err(error("`style = two_args` requires two positional-only parameters"));
    };
    if params.iter().count() != 2
        || params.iter().any(|p| p.annotation().is_some() || p.default().is_some())
        || declaration.owner.is_some()
        || declaration.kwarg_error_name.is_some()
    {
        return Err(error(
            "`style = two_args` requires two raw values and uses the callable name for errors",
        ));
    }
    let first = field_name(&first.parameter, literal)?;
    let second = field_name(&second.parameter, literal)?;
    let wanted_pat: Pat = parse_quote!((#first, #second));
    let wanted_ty: Type = parse_quote!((&Value, &Value));
    let arg = function
        .sig
        .inputs
        .iter_mut()
        .find_map(|input| match input {
            FnArg::Typed(arg) if *arg.pat == wanted_pat && *arg.ty == wanted_ty => Some(arg),
            _ => None,
        })
        .ok_or_else(|| error("expected a pair of &Value parameters in signature order"))?;
    arg.pat = parse_quote!(__py_args);
    arg.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({
        let (#first, #second) = __py_args.unpack_two_args(#name, vm.heap)?;
        crate::defer_drop!(#first, vm);
        crate::defer_drop!(#second, vm);
        #body
    }));
    Ok(quote!(#function))
}

/// Bind the object with the same arity check and drop guard as a handwritten one-argument builtin.
fn single_positional_adapter(mut function: ItemFn, name: &str, field: &Ident) -> syn::Result<TokenStream> {
    let arg = function
        .sig
        .inputs
        .iter_mut()
        .find_map(|input| {
            let FnArg::Typed(arg) = input else { return None };
            matches!(arg.pat.as_ref(), Pat::Ident(pattern) if pattern.ident == *field).then_some(arg)
        })
        .ok_or_else(|| {
            syn::Error::new(
                field.span(),
                format!("expected a parameter named `{field}` of type &Value"),
            )
        })?;
    if !matches!(arg.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_none()
        && matches!(reference.elem.as_ref(), Type::Path(path) if path.path.is_ident("Value")))
    {
        return Err(syn::Error::new_spanned(
            &arg.ty,
            "expected &Value for a single positional argument",
        ));
    }
    *arg.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({
        let #field = #field.get_one_arg(#name, vm.heap)?;
        crate::defer_drop!(#field, vm);
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
    let converter = declaration
        .converters
        .iter()
        .find(|(field, _)| field == &ident)
        .map(|(_, ty)| ty);
    if let Some(ty) = converter {
        if annotation.is_some() {
            return Err(error("a custom converter supplies the parameter type"));
        }
        let (ty, default) = match param.default() {
            None => (quote!(#ty), quote!()),
            Some(Expr::EllipsisLiteral(_)) => (quote!(Option<#ty>), quote!(default)),
            Some(expr) if let Some(n) = integer_default(expr) => (quote!(#ty), quote!(default = <#ty>::from(#n))),
            _ => return Err(error("unsupported custom converter default")),
        };
        return Ok(quote!(#[from_args(#role #default)] #ident: #ty));
    }
    let ty = match annotation {
        None if matches!(param.default(), Some(Expr::EllipsisLiteral(_))) => quote!(Option<crate::value::Value>),
        None if matches!(param.default(), Some(Expr::StringLiteral(_))) => quote!(crate::args::DefaultedStringArg),
        None => quote!(crate::value::Value),

        Some("int") => quote!(i32),
        Some("bool") => quote!(crate::args::LaxBool),
        Some("str") => quote!(crate::args::StrArg),
        Some("str | None") => quote!(Option<crate::args::StrArg>),
        Some("float") => quote!(f64),
        Some("SupportsIndex") => quote!(i64),
        Some(_) => return Err(error("unsupported native parameter annotation")),
    };
    let ty = if annotation.is_some() && matches!(param.default(), Some(Expr::EllipsisLiteral(_))) {
        quote!(Option<#ty>)
    } else {
        ty
    };
    let default = match (annotation, param.default()) {
        (_, None) => quote!(),
        (None | Some("str" | "int"), Some(Expr::EllipsisLiteral(_))) => quote!(default),
        (Some("str | None"), Some(Expr::NoneLiteral(_))) => quote!(default),
        (None, Some(Expr::NoneLiteral(_))) => quote!(default = crate::value::Value::None),
        (Some("bool"), Some(Expr::BooleanLiteral(b))) => {
            let value = b.value;
            quote!(default = crate::args::LaxBool::new(#value))
        }
        (Some("int"), Some(expr)) => {
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
        (Some("SupportsIndex"), Some(expr)) => {
            let n = integer_default(expr).ok_or_else(|| error("expected an i64 literal for SupportsIndex"))?;
            quote!(default = #n)
        }
        _ => return Err(error("unsupported default for this parameter type")),
    };
    let extractor = match annotation {
        Some("SupportsIndex") => quote!(extract_with = crate::args::extract_index,),
        Some("str | None") => quote!(extract_with = crate::args::extract_nullable_str,),
        _ => quote!(),
    };
    Ok(quote!(#[from_args(#role #extractor #default)] #ident: #ty))
}

fn integer_default(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::NumberLiteral(n) => match &n.value {
            Number::Int(i) => i.as_i64(),
            _ => None,
        },
        Expr::UnaryOp(op) if op.op == UnaryOp::USub => integer_default(&op.operand)?.checked_neg(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../tests/unit/py_signature.rs"]
mod tests;
