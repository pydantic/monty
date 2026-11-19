//! Parses native signatures with the same Python grammar as interpreted functions.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use ruff_python_ast::{CmpOp, Expr, Number, Parameter, ParameterWithDefault, Stmt, StmtFunctionDef, UnaryOp};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;
use syn::{
    DeriveInput, FnArg, ItemFn, LitStr, Pat, Token,
    parse::{Parse, ParseStream},
    parse_quote,
};

use crate::from_args::Signature;

pub(super) enum Declaration {
    Inline(LitStr),
    Typeshed(LitStr),
}

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(LitStr) {
            return input.parse().map(Self::Inline);
        }
        let key: syn::Ident = input.parse()?;
        if key != "typeshed" {
            return Err(syn::Error::new(key.span(), "expected typeshed or a Python declaration"));
        }
        input.parse::<Token![=]>()?;
        input.parse().map(Self::Typeshed)
    }
}

pub(super) fn expand(declaration: &Declaration, function: ItemFn) -> syn::Result<TokenStream> {
    let (literal, source) = match declaration {
        Declaration::Inline(literal) => (literal, format!("def {}: pass", literal.value())),
        Declaration::Typeshed(literal) => {
            let builtins = monty_typeshed::file_system()
                .read_to_string("stdlib/builtins.pyi")
                .map_err(|e| syn::Error::new(literal.span(), e.to_string()))?;
            (literal, builtins)
        }
    };
    let error = |message: &str| syn::Error::new(literal.span(), message);
    let parsed = parse_module(&source).map_err(|e| error(&e.to_string()))?;
    let stmts = &parsed.syntax().body;
    let def = match declaration {
        Declaration::Inline(_) => match &stmts[..] {
            [Stmt::FunctionDef(def)] if matches!(&def.body[..], [Stmt::Pass(_)]) => def,
            _ => return Err(error("expected one Python signature")),
        },
        Declaration::Typeshed(_) => {
            let path = literal.value();
            let (class, method) = path.split_once('.').ok_or_else(|| error("expected Class.method"))?;
            let class = stmts
                .iter()
                .filter_map(Stmt::as_class_def_stmt)
                .find(|c| c.name.as_str() == class)
                .ok_or_else(|| error("class missing from builtins.pyi"))?;
            let mut methods = Vec::new();
            find_methods(&class.body, method, &mut methods).map_err(error)?;
            match methods.as_slice() {
                [def] => *def,
                _ => return Err(error("expected one general overload in builtins.pyi")),
            }
        }
    };
    generate(literal, &source, def, function)
}

// Use the general overload. Overloads that specialize the receiver (e.g. LiteralString)
// describe a more precise return type to the static checker.
fn find_methods<'a>(stmts: &'a [Stmt], method: &str, found: &mut Vec<&'a StmtFunctionDef>) -> Result<(), &'static str> {
    for stmt in stmts {
        match stmt {
            Stmt::FunctionDef(def)
                if def.name.as_str() == method
                    && def
                        .parameters
                        .posonlyargs
                        .first()
                        .is_some_and(|p| p.name().as_str() == "self" && p.annotation().is_none()) =>
            {
                found.push(def);
            }
            Stmt::If(branch) => {
                let mut yes = Vec::new();
                let mut no = Vec::new();
                find_methods(&branch.body, method, &mut yes)?;
                for clause in &branch.elif_else_clauses {
                    find_methods(&clause.body, method, &mut no)?;
                }
                if !yes.is_empty() || !no.is_empty() {
                    if branch.elif_else_clauses.iter().any(|clause| clause.test.is_some()) {
                        return Err("unsupported elif around native overloads");
                    }
                    let active =
                        version_guard(&branch.test).ok_or("unsupported version guard around native overloads")?;
                    found.extend(if active { yes } else { no });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn version_guard(expr: &Expr) -> Option<bool> {
    let comparison = expr.as_compare_expr()?;
    let [Expr::Attribute(attr), Expr::Tuple(version)] = &*comparison.operands else {
        return None;
    };
    if attr.value.as_name_expr()?.id.as_str() != "sys" || attr.attr.as_str() != "version_info" {
        return None;
    }
    let [major, minor] = &version.elts[..] else { return None };
    let version = (integer_default(major)?, integer_default(minor)?);
    match &*comparison.ops {
        [CmpOp::GtE] => Some((3, 14) >= version),
        [CmpOp::Lt] => Some((3, 14) < version),
        _ => None,
    }
}

fn generate(literal: &LitStr, source: &str, def: &StmtFunctionDef, mut function: ItemFn) -> syn::Result<TokenStream> {
    let error = |message: &str| syn::Error::new(literal.span(), message);
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
    if params.kwarg.is_some() {
        return Err(error("variadic keyword parameters are unsupported"));
    }

    let Some(FnArg::Typed(bound)) = function.sig.inputs.last_mut() else {
        return Err(error("expected the bound arguments as the final Rust parameter"));
    };
    let Pat::Ident(pattern) = &*bound.pat else {
        return Err(error("expected a name for the bound arguments"));
    };
    let arg = pattern.ident.clone();
    let bound_ty = bound.ty.clone();
    let syn::Type::Reference(reference) = &*bound_ty else {
        return Err(error("expected a reference to the generated argument struct"));
    };
    let syn::Type::Path(path) = &*reference.elem else {
        return Err(error("expected a bound argument type name"));
    };
    let bound_name = path
        .path
        .get_ident()
        .ok_or_else(|| error("expected a local argument type name"))?;
    let style = receiver.then(|| quote!(, bad_arg));
    let input: DeriveInput = syn::parse2(quote! {
        #[from_args(name = #name #style)]
        struct #bound_name { #(#fields,)* }
    })?;
    let signature = Signature::parse(&input)?;
    let binding = signature.binding_spec();
    let descriptor = format_ident!("{}_SIGNATURE", function.sig.ident.to_string().to_uppercase());
    let from_args = signature.render(Some(quote!(&#descriptor.binding)));
    let mut bound_struct = input;
    bound_struct.attrs.clear();
    if let syn::Data::Struct(data) = &mut bound_struct.data {
        for field in &mut data.fields {
            field.attrs.clear();
        }
    }

    *bound.ty = parse_quote!(crate::args::ArgValues);
    let body = function.block;
    function.block = Box::new(parse_quote!({ vm.with_args(#arg, |#arg: #bound_ty, vm| #body) }));
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
    } else {
        vec!["$module".to_owned()]
    };
    text.extend(params.posonlyargs.iter().skip(usize::from(receiver)).map(display_param));
    text.push("/".to_owned());
    text.extend(params.args.iter().map(display_param));
    if let Some(vararg) = &params.vararg {
        text.push(format!("*{}", vararg.name));
    } else if !params.kwonlyargs.is_empty() {
        text.push("*".to_owned());
    }
    text.extend(params.kwonlyargs.iter().map(display_param));
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
        _ => return Err(error("expected a simple native parameter type")),
    };
    let ty = match annotation {
        None if matches!(param.default(), Some(Expr::StringLiteral(_))) => quote!(crate::args::DefaultedStringArg),
        None => quote!(crate::value::Value),
        Some("str") => quote!(crate::args::StrArg),
        Some("SupportsIndex") => quote!(crate::args::IndexArg),
        Some(_) => return Err(error("unsupported native parameter annotation")),
    };
    let default = match (annotation, param.default()) {
        (_, None) => quote!(),
        (None, Some(Expr::NoneLiteral(_))) => quote!(default = crate::value::Value::None),
        (None, Some(Expr::BooleanLiteral(b))) => {
            let value = b.value;
            quote!(default = crate::value::Value::Bool(#value))
        }
        (None, Some(Expr::StringLiteral(s))) => {
            let value = s.value.to_str();
            quote!(default = crate::args::DefaultedStringArg::Omitted(#value))
        }
        (Some("SupportsIndex"), Some(expr)) => {
            let n = integer_default(expr).ok_or_else(|| error("expected an i64 literal for SupportsIndex"))?;
            quote!(default = crate::args::IndexArg(#n))
        }
        _ => return Err(error("unsupported default for this parameter type")),
    };
    Ok(quote!(#[from_args(#role #default)] #ident: #ty))
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
