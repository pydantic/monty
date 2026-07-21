//! Rendering a ruff AST expression back to source text, the way CPython's
//! PEP 563 stringizer does.
//!
//! The inverse of [`parse`](crate::parse): that module turns source into Monty's
//! IR, this one turns a fragment of the AST back into the string a Python program
//! would observe. Annotations are the only caller today — they are stored
//! stringized rather than evaluated (see `limitations/typing.md`) — but the
//! contract is "produce exactly what CPython's stringizer would", so any future
//! stringization (function or module `__annotations__`) belongs here too.
//!
//! Unparsing rather than slicing the source is what makes `x: dict[str,int]`
//! yield `'dict[str, int]'` on both interpreters; a slice would leak the original
//! spacing, line breaks and quote style into the value.

use ruff_python_ast::{
    self as ast, AtomicNodeIndex, Expr as AstExpr,
    str::{Quote, TripleQuotes},
    str_prefix::StringLiteralPrefix,
    visitor::transformer::{Transformer, walk_expr, walk_f_string},
};
use ruff_python_codegen::{Generator, Indentation};
use ruff_source_file::LineEnding;

/// Renders `annotation` to the text CPython's PEP 563 stringizer would produce.
///
/// Takes `&mut` because canonicalising the literals rewrites them in place; the
/// expression is not otherwise used after stringization.
///
/// ```ignore
/// stringize_annotation(&mut expr)  // `dict[str,"Foo"]` -> `dict[str, 'Foo']`
/// ```
pub(crate) fn stringize_annotation(annotation: &mut AstExpr) -> String {
    // Neither generator mode matches CPython alone: `Mode::AstUnparse` normalises
    // quotes but parenthesises tuple subscripts (`dict[(str, int)]`), while
    // `Mode::Default` keeps subscripts but echoes the source literals. So use
    // `Default` and canonicalise the literals first.
    CanonicalStringLiterals.visit_expr(annotation);
    // `LineEnding::Lf` is pinned rather than defaulted: the default is
    // platform-dependent, and Monty must stringize identically everywhere.
    Generator::new(&Indentation::default(), LineEnding::Lf).expr(annotation)
}

/// Rewrites the string literals in an expression into the single canonical form
/// CPython's stringizer produces, so unparsing matches it.
///
/// The generator echoes each literal roughly as written — every implicitly
/// concatenated part separately, prefixes and triple quotes intact — where
/// CPython rebuilds *one* literal from the value. Descends the whole expression,
/// so `dict[str, "Foo"]` is normalised at any depth.
///
/// | annotation     | without this   | with it (= CPython) |
/// | -------------- | -------------- | ------------------- |
/// | `"foo" "bar"`  | `'foo' 'bar'`  | `'foobar'`          |
/// | `f"x" "y"`     | `f'x' 'y'`     | `f'xy'`             |
/// | `r"raw\d"`     | `r'raw\d'`     | `'raw\\d'`          |
/// | `"""triple"""` | `"""triple"""` | `'triple'`          |
struct CanonicalStringLiterals;

impl Transformer for CanonicalStringLiterals {
    fn visit_string_literal(&self, string_literal: &mut ast::StringLiteral) {
        string_literal.flags = canonical_string_flags(string_literal.flags);
    }

    // F-strings carry their own flags, so they need canonicalising separately;
    // walking the elements still reaches any literals nested in a format spec.
    fn visit_f_string(&self, f_string: &mut ast::FString) {
        f_string.flags = f_string
            .flags
            .with_quote_style(Quote::Single)
            .with_triple_quotes(TripleQuotes::No);
        walk_f_string(self, f_string);
    }

    // Concatenation is a property of the *expression*, not of any one part, so
    // the merge happens here and the visits above then canonicalise the result.
    fn visit_expr(&self, expr: &mut AstExpr) {
        match expr {
            AstExpr::StringLiteral(s) if s.value.is_implicit_concatenated() => merge_string_parts(s),
            AstExpr::FString(f) if f.value.is_implicit_concatenated() => merge_f_string_parts(f),
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// The flags CPython's stringizer effectively renders a `str` literal with:
/// single-quoted and not triple-quoted, with a raw prefix folded into the value
/// (`r"raw\d"` becomes `'raw\\d'`) since the generator re-escapes what the raw
/// prefix used to cover.
///
/// A `u` prefix is *kept*: CPython renders `u"x"` as `u'x'`, so canonical is not
/// the same as bare. The quote style is a request, not a guarantee — the
/// generator still switches to double quotes when the value contains a single
/// one, which is what CPython does too.
fn canonical_string_flags(flags: ast::StringLiteralFlags) -> ast::StringLiteralFlags {
    let prefix = match flags.prefix() {
        StringLiteralPrefix::Unicode => StringLiteralPrefix::Unicode,
        StringLiteralPrefix::Empty | StringLiteralPrefix::Raw { .. } => StringLiteralPrefix::Empty,
    };
    flags
        .with_prefix(prefix)
        .with_quote_style(Quote::Single)
        .with_triple_quotes(TripleQuotes::No)
}

/// Collapses `"foo" "bar"` into the single literal `'foobar'`.
///
/// The parts' own flags are discarded in favour of the first part's, matching
/// CPython: the concatenated *value* is what survives, and `visit_string_literal`
/// canonicalises the flags immediately afterwards.
fn merge_string_parts(expr: &mut ast::ExprStringLiteral) {
    let merged = ast::StringLiteral {
        range: expr.range,
        node_index: AtomicNodeIndex::default(),
        value: expr.value.to_str().into(),
        flags: expr.value.first_literal_flags(),
    };
    expr.value = ast::StringLiteralValue::single(merged);
}

/// Collapses `f"x" "y"` into the single f-string `f'xy'`.
///
/// Unlike the `str` case the parts are not all the same kind — a concatenation
/// mixes plain literals with f-strings — so the merge splices their *elements*
/// into one element list rather than joining a string. A plain part contributes
/// one literal element; an f-string part contributes all of its own.
fn merge_f_string_parts(expr: &mut ast::ExprFString) {
    let mut elements: Vec<ast::InterpolatedStringElement> = Vec::new();
    // The first f-string part's flags stand for the whole, as above.
    let mut flags = None;
    for part in &expr.value {
        match part {
            ast::FStringPart::Literal(literal) => {
                elements.push(ast::InterpolatedStringElement::Literal(
                    ast::InterpolatedStringLiteralElement {
                        range: literal.range,
                        node_index: AtomicNodeIndex::default(),
                        value: literal.value.clone(),
                    },
                ));
            }
            ast::FStringPart::FString(f_string) => {
                flags = flags.or(Some(f_string.flags));
                elements.extend(f_string.elements.iter().cloned());
            }
        }
    }
    // `is_implicit_concatenated` guarantees at least one f-string part, since a
    // concatenation with none would not have parsed as an f-string.
    let Some(flags) = flags else { return };
    expr.value = ast::FStringValue::single(ast::FString {
        range: expr.range,
        node_index: AtomicNodeIndex::default(),
        elements: elements.into(),
        flags,
    });
}
