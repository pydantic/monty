//! In-place signature binding for native dispatchers that own their arguments.

use proc_macro2::TokenStream;
use quote::quote;
use ruff_python_ast::Expr as PyExpr;
use syn::{
    Expr, Ident, LitStr, Token,
    parse::{Parse, ParseStream},
};

use crate::py_signature::parse_signature_source;

pub(super) struct Input {
    args: Expr,
    heap: Expr,
    signature: LitStr,
    name: Option<Expr>,
    iter: bool,
    reject_keywords: bool,
}

impl Parse for Input {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let args = input.parse()?;
        input.parse::<Token![,]>()?;
        let heap = input.parse()?;
        input.parse::<Token![,]>()?;
        let signature = input.parse()?;
        let mut binding = Self {
            args,
            heap,
            signature,
            name: None,
            iter: false,
            reject_keywords: false,
        };
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let option: Ident = input.parse()?;
            if option == "name" && binding.name.is_none() {
                input.parse::<Token![=]>()?;
                binding.name = Some(input.parse()?);
            } else if option == "iter" && !binding.iter {
                binding.iter = true;
            } else if option == "reject_keywords" && !binding.reject_keywords {
                binding.reject_keywords = true;
            } else {
                return Err(syn::Error::new(
                    option.span(),
                    "unknown or repeated positional binding option",
                ));
            }
        }
        Ok(binding)
    }
}

impl Input {
    pub(super) fn expand(self) -> syn::Result<TokenStream> {
        let Self {
            args,
            heap,
            signature,
            name,
            iter,
            reject_keywords,
        } = self;
        let literal = signature.value();
        let error = |message| syn::Error::new(signature.span(), message);
        let (qualified, params) = literal
            .split_once('(')
            .ok_or_else(|| error("expected a Python signature"))?;
        let callable = match (qualified, name) {
            ("", Some(expr)) => quote!(#expr),
            ("", None) => return Err(error("a parameter list needs a callable name")),
            (_, Some(_)) => return Err(error("specify the callable name only once")),
            (_, None) => quote!(#qualified),
        };
        let function = parse_signature_source(&signature, &format!("def _native({params}: pass"))?;
        if function.returns.is_some() {
            return Err(error("a positional binding cannot have a return annotation"));
        }
        let params = function.parameters;
        if !params.args.is_empty()
            || !params.kwonlyargs.is_empty()
            || params.kwarg.is_some()
            || params.iter().any(|p| p.annotation().is_some())
        {
            return Err(error("expected unannotated positional-only parameters or *args"));
        }
        let max = params.posonlyargs.len();
        let min = params.posonlyargs.iter().take_while(|p| p.default().is_none()).count();
        if params
            .posonlyargs
            .iter()
            .skip(min)
            .any(|p| !matches!(p.default(), Some(PyExpr::EllipsisLiteral(_))))
        {
            return Err(error("use =... to bind an omitted positional argument as None"));
        }
        if reject_keywords && (iter || params.vararg.is_some() || max == 0) {
            return Err(error("this binding already checks keywords first"));
        }
        let bind = if params.vararg.is_some() {
            if iter {
                return Err(error("*args already returns an iterator"));
            }
            if max != 0 {
                return Err(error("use py_signature to combine named parameters and *args"));
            }
            quote!(__args.into_pos_only(__name, __heap))
        } else if iter {
            quote!(__args.bind_positional_range(__name, #min, #max, __heap))
        } else {
            match (min, max) {
                (0, 0) => quote!(__args.check_zero_args(__name, __heap)),
                (1, 1) => quote!(__args.get_one_arg(__name, __heap)),
                (2, 2) => quote!(__args.get_two_args(__name, __heap)),
                (0, 1) => quote!(__args.get_zero_one_arg(__name, __heap)),
                (1, 2) => quote!(__args.get_one_two_args(__name, __heap)),
                _ => return Err(error("use iter to bind a larger positional signature")),
            }
        };
        let bind = if reject_keywords {
            quote!(__args.reject_kwargs(__name, __heap).and_then(|__args| #bind))
        } else {
            bind
        };
        Ok(quote!({
            let (__args, __name, __heap): (_, &str, _) = (#args, #callable, &mut *(#heap));
            #bind
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::Input;

    #[test]
    fn rejects_signatures_that_cannot_be_bound() {
        for input in [
            r#"args, heap, "(x, /)""#,
            r#"args, heap, "f(x, /)", name = other"#,
            r#"args, heap, "f(x)""#,
            r#"args, heap, "f(*, x)""#,
            r#"args, heap, "f(**kwargs)""#,
            r#"args, heap, "f(x: int, /)""#,
            r#"args, heap, "f(x=0, /)""#,
            r#"args, heap, "f(x, /, *rest)""#,
            r#"args, heap, "f() -> int""#,
            r#"args, heap, "f(x, y, z, /)""#,
            r#"args, heap, "f(*args)", iter"#,
            r#"args, heap, "f()", reject_keywords"#,
            r#"args, heap, "f()", iter, iter"#,
        ] {
            assert!(
                syn::parse_str::<Input>(input).and_then(Input::expand).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn selects_the_inline_storage_for_each_shape() {
        for (signature, helper) in [
            ("f()", "check_zero_args"),
            ("f(x, /)", "get_one_arg"),
            ("f(x, y, /)", "get_two_args"),
            ("f(x=..., /)", "get_zero_one_arg"),
            ("f(x, y=..., /)", "get_one_two_args"),
            ("f(*xs)", "into_pos_only"),
        ] {
            let input: Input = syn::parse_str(&format!("args, heap, {signature:?}")).unwrap();
            assert!(input.expand().unwrap().to_string().contains(helper), "{signature}");
        }
    }
}
