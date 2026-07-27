//! Rendering a ruff AST expression back to source text, the way CPython's
//! PEP 563 stringizer does.
//!
//! The inverse of [`parse`](crate::parse). Class annotations are the only caller
//! today (they are stored stringized, not evaluated — see
//! `limitations/typing.md`), but the contract is "match CPython's stringizer", so
//! any future stringization belongs here. Unparsing rather than slicing the
//! source is what makes `x: dict[str,int]` yield `'dict[str, int]'` on both.

use ruff_python_ast::{
    self as ast, AtomicNodeIndex, Expr as AstExpr,
    str::{Quote, TripleQuotes},
    str_prefix::{ByteStringPrefix, StringLiteralPrefix},
    visitor::transformer::{Transformer, walk_expr, walk_f_string},
};
use ruff_python_codegen::{Generator, Indentation};
use ruff_source_file::LineEnding;

/// Renders `annotation` to the text CPython's PEP 563 stringizer would produce.
///
/// Takes `&mut` because canonicalising the literals rewrites them in place.
pub(crate) fn stringize_annotation(annotation: &mut AstExpr) -> String {
    // Neither generator mode matches CPython alone: `AstUnparse` normalises quotes
    // but parenthesises tuple subscripts (`dict[(str, int)]`), `Default` keeps
    // subscripts but echoes the source literals. So canonicalise, then `Default`.
    CanonicalStringLiterals.visit_expr(annotation);
    // `Lf` is pinned because the default is platform-dependent, and Monty must
    // stringize identically everywhere.
    Generator::new(&Indentation::default(), LineEnding::Lf).expr(annotation)
}

/// Rewrites string literals into the single canonical form CPython produces,
/// since the generator otherwise echoes them roughly as written.
///
/// | annotation     | without this   | with it (= CPython) |
/// | -------------- | -------------- | ------------------- |
/// | `"foo" "bar"`  | `'foo' 'bar'`  | `'foobar'`          |
/// | `f"x" "y"`     | `f'x' 'y'`     | `f'xy'`             |
/// | `r"raw\d"`     | `r'raw\d'`     | `'raw\\d'`          |
/// | `"""triple"""` | `"""triple"""` | `'triple'`          |
/// | `b"foo" b"bar"`| `b"foo" b"bar"`| `b'foobar'`         |
struct CanonicalStringLiterals;

impl Transformer for CanonicalStringLiterals {
    // Rebuilding is unconditional: the value is the truth and the spelling noise,
    // so there is no unaffected case to preserve.
    fn visit_expr(&self, expr: &mut AstExpr) {
        match expr {
            AstExpr::StringLiteral(s) => rebuild_string_literal(s),
            AstExpr::BytesLiteral(b) => rebuild_bytes_literal(b),
            AstExpr::FString(f) if f.value.is_implicit_concatenated() => merge_f_string_parts(f),
            _ => {}
        }
        walk_expr(self, expr);
    }

    // F-strings cannot be rebuilt from a value — the interpolations are live
    // expressions — so only the flags are canonicalised.
    fn visit_f_string(&self, f_string: &mut ast::FString) {
        f_string.flags = f_string
            .flags
            .with_quote_style(Quote::Single)
            .with_triple_quotes(TripleQuotes::No);
        walk_f_string(self, f_string);
    }
}

/// The flags CPython effectively renders a `str` literal with: single-quoted, not
/// triple-quoted, raw prefix folded into the value (the generator re-escapes what
/// it covered). Quote style is a request — the generator still switches to double
/// quotes when the value contains a single one, as CPython does.
///
/// `u` is kept because it is the only prefix CPython's AST retains (as
/// `Constant.kind`); raw-ness is consumed by its parser and cannot come back.
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

/// Replaces a `str` literal with the canonical literal for its value, collapsing
/// `"foo" "bar"` into `'foobar'`.
///
/// CPython needs no equivalent: its parser already folded concatenation and
/// consumed the prefix, so its unparser just writes `repr(value)`. Ruff keeps the
/// spelling — a formatter needs it — so the discard happens here.
fn rebuild_string_literal(expr: &mut ast::ExprStringLiteral) {
    let canonical = ast::StringLiteral {
        range: expr.range,
        node_index: AtomicNodeIndex::default(),
        // `to_str` concatenates every part, and is the identity for one.
        value: expr.value.to_str().into(),
        flags: canonical_string_flags(expr.value.first_literal_flags()),
    };
    expr.value = ast::StringLiteralValue::single(canonical);
}

/// The `bytes` counterpart of [`rebuild_string_literal`], collapsing
/// `b"foo" b"bar"` into `b'foobar'`.
///
/// Simpler than the `str` case only in the flags: `bytes` has no `u` prefix, so
/// the canonical form keeps no prefix at all.
fn rebuild_bytes_literal(expr: &mut ast::ExprBytesLiteral) {
    let canonical = ast::BytesLiteral {
        range: expr.range,
        node_index: AtomicNodeIndex::default(),
        value: expr.value.bytes().collect(),
        flags: ast::BytesLiteralFlags::empty()
            .with_prefix(ByteStringPrefix::Regular)
            .with_quote_style(Quote::Single)
            .with_triple_quotes(TripleQuotes::No),
    };
    expr.value = ast::BytesLiteralValue::single(canonical);
}

/// Collapses `f"x" "y"` into the single f-string `f'xy'`.
///
/// Splices element lists rather than joining a string, because the parts are not
/// all the same kind: a plain part contributes one literal element, an f-string
/// part contributes all of its own.
fn merge_f_string_parts(expr: &mut ast::ExprFString) {
    let mut elements: Vec<ast::InterpolatedStringElement> = Vec::new();
    // The first f-string part's flags stand for the whole.
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
    // Unreachable: a concatenation with no f-string part would not parse as one.
    let Some(flags) = flags else { return };
    expr.value = ast::FStringValue::single(ast::FString {
        range: expr.range,
        node_index: AtomicNodeIndex::default(),
        elements: elements.into(),
        flags,
    });
}
