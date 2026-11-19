//! Parses native signatures with the same Python grammar as interpreted functions.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use ruff_python_ast::{Expr, Number, Operator, Parameter, ParameterWithDefault, Stmt, UnaryOp};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;
use syn::{
    DeriveInput, FnArg, Ident, ItemFn, LitStr, Pat, Token, Type,
    meta::parser,
    parse::{Parse, ParseStream, Parser},
    parse_quote,
};

use crate::from_args::{Signature, snake_to_pascal};

pub(super) struct Declaration {
    signature: LitStr,
    style: Option<Ident>,
    owner: Option<LitStr>,
    kwarg_error_name: Option<LitStr>,
    check_last: Option<Ident>,
}

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut declaration = Self {
            signature: input.parse()?,
            style: None,
            owner: None,
            kwarg_error_name: None,
            check_last: None,
        };
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
            parser(|meta| {
                if meta.path.is_ident("style") {
                    declaration.style = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("owner") {
                    declaration.owner = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("kwarg_error_name") {
                    declaration.kwarg_error_name = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("check_last") {
                    declaration.check_last = Some(meta.value()?.parse()?);
                } else {
                    return Err(meta.error("unknown py_signature option"));
                }
                Ok(())
            })
            .parse2(input.parse()?)?;
        }
        Ok(declaration)
    }
}

pub(super) fn expand(declaration: &Declaration, mut function: ItemFn) -> syn::Result<TokenStream> {
    let literal = &declaration.signature;
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let source = format!("def {}: pass", literal.value());
    let parsed = parse_module(&source).map_err(|e| error(&e.to_string()))?;
    let [Stmt::FunctionDef(def)] = &parsed.syntax().body[..] else {
        return Err(error("expected one Python function signature"));
    };
    if !matches!(&def.body[..], [Stmt::Pass(_)]) {
        return Err(error("expected a Python function signature without a body"));
    }
    let name = def.name.as_str();
    let params = &def.parameters;
    let receiver = params.posonlyargs.first().is_some_and(|p| p.name().as_str() == "self");
    let mut fields = Vec::new();
    for param in params.posonlyargs.iter().skip(usize::from(receiver)) {
        fields.push(named_field(param, &quote!(pos_only,), literal)?);
    }
    for param in &params.args {
        fields.push(named_field(param, &quote!(), literal)?);
    }
    if let Some(rest) = &params.vararg {
        let ident = field_name(rest, literal)?;
        if rest.annotation.is_some() {
            return Err(error("typed variadic parameters are unsupported"));
        }
        fields.push(quote!(#[from_args(varargs)] #ident: Vec<crate::value::Value>));
    }
    for param in &params.kwonlyargs {
        fields.push(named_field(param, &quote!(kw_only,), literal)?);
    }
    if let Some(kwargs) = &params.kwarg {
        let ident = field_name(kwargs, literal)?;
        if kwargs.annotation.is_some() {
            return Err(error("typed variadic parameters are unsupported"));
        }
        fields.push(quote!(#[from_args(varkwargs)] #ident: crate::args::KwargsValues));
    }

    let bound_name = format_ident!("{}Args", snake_to_pascal(name));
    let bound = function
        .sig
        .inputs
        .iter_mut()
        .find_map(|input| {
            let FnArg::Typed(arg) = input else { return None };
            let Type::Reference(reference) = arg.ty.as_ref() else {
                return None;
            };
            let Type::Path(path) = reference.elem.as_ref() else {
                return None;
            };
            path.path.is_ident(&bound_name).then_some(arg)
        })
        .ok_or_else(|| error(&format!("expected a parameter of type &{bound_name}")))?;
    let Pat::Ident(pattern) = &*bound.pat else {
        return Err(error("expected a name for the bound arguments"));
    };
    let arg = pattern.ident.clone();
    let bound_ty = bound.ty.clone();
    let with_args = if matches!(&*bound_ty, Type::Reference(reference) if reference.mutability.is_some()) {
        quote!(with_args_mut)
    } else {
        quote!(with_args)
    };
    if declaration.owner.is_some() && !receiver {
        return Err(error("owner requires a positional-only self parameter"));
    }
    let qualified_name = declaration
        .owner
        .as_ref()
        .map_or_else(|| name.to_owned(), |owner| format!("{}.{name}", owner.value()));
    let style = declaration.style.as_ref().map(|style| quote!(, style = #style));
    let is_def = declaration.style.as_ref().is_some_and(|style| style == "def");
    let bad_arg = (receiver && !is_def).then(|| quote!(, bad_arg));
    let kwarg_error = declaration
        .kwarg_error_name
        .as_ref()
        .map(|name| quote!(, kwarg_error_name = #name));
    let input: DeriveInput = syn::parse2(quote! {
        #[from_args(name = #qualified_name #style #bad_arg #kwarg_error)]
        struct #bound_name { #(#fields,)* }
    })?;
    let signature = Signature::parse(&input)?;
    let binding = signature.binding_spec(receiver);
    let descriptor = format_ident!("{}_SIGNATURE", function.sig.ident.to_string().to_uppercase());
    let from_args = signature.render_native(quote!(&#descriptor.binding), declaration.check_last.as_ref())?;
    let mut bound_struct = input;
    bound_struct.attrs.clear();
    if let syn::Data::Struct(data) = &mut bound_struct.data {
        for field in &mut data.fields {
            field.attrs.clear();
        }
    }

    *bound.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({ vm.#with_args(#arg, |#arg: #bound_ty, vm| #body) }));
    let display_param = |p: &ParameterWithDefault| {
        let mut text = p.name().as_str().to_owned();
        if let Some(default) = p.default() {
            text.push('=');
            text.push_str(&source[default.range()]);
        }
        text
    };
    let mut text = if receiver {
        vec!["$self".to_owned()]
    } else if is_def {
        Vec::new()
    } else {
        vec!["$module".to_owned()]
    };
    text.extend(params.posonlyargs.iter().skip(usize::from(receiver)).map(display_param));
    if !text.is_empty() {
        text.push("/".to_owned());
    }
    text.extend(params.args.iter().map(display_param));
    if let Some(vararg) = &params.vararg {
        text.push(format!("*{}", vararg.name));
    } else if !params.kwonlyargs.is_empty() {
        text.push("*".to_owned());
    }
    text.extend(params.kwonlyargs.iter().map(display_param));
    if let Some(kwargs) = &params.kwarg {
        text.push(format!("**{}", kwargs.name));
    }
    let text_signature = format!("({})", text.join(", "));
    Ok(quote! {
        pub(crate) static #descriptor: crate::args::NativeSignature = crate::args::NativeSignature {
            binding: #binding,
            text_signature: #text_signature,
        };
        #bound_struct
        #from_args
        #function
    })
}

fn field_name(param: &Parameter, literal: &LitStr) -> syn::Result<syn::Ident> {
    syn::parse_str(param.name.as_str())
        .map_err(|_| syn::Error::new(literal.span(), "parameter name must also be a Rust identifier"))
}

fn named_field(param: &ParameterWithDefault, role: &TokenStream, literal: &LitStr) -> syn::Result<TokenStream> {
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
    let ty = match annotation {
        None if matches!(param.default(), Some(Expr::EllipsisLiteral(_))) => quote!(Option<crate::value::Value>),
        None if matches!(param.default(), Some(Expr::StringLiteral(_))) => quote!(crate::args::DefaultedStringArg),
        None => quote!(crate::value::Value),
        Some("str") => quote!(crate::args::StrArg),
        Some("str | None") => quote!(Option<crate::args::StrArg>),
        Some("float") => quote!(f64),
        Some("SupportsIndex") => quote!(i64),
        Some(_) => return Err(error("unsupported native parameter annotation")),
    };
    let default = match (annotation, param.default()) {
        (_, None) => quote!(),
        (None, Some(Expr::EllipsisLiteral(_))) => quote!(default),
        (Some("str | None"), Some(Expr::NoneLiteral(_))) => quote!(default),
        (None, Some(Expr::NoneLiteral(_))) => quote!(default = crate::value::Value::None),
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
