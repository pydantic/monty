//! Pre-parse bound on how deeply ruff's parser may recurse on long sources.
//!
//! ruff grows its parser stack on demand (via `stacker`), in segments the
//! sandbox allocator never sees, so a source of N bytes could otherwise cost
//! O(N) untracked memory before Monty's own nesting check
//! ([`MAX_NESTING_DEPTH`]) ever runs. One lexer pass over-approximates the
//! parser's recursion depth; a source of at most `source_scan_threshold` bytes
//! skips it, its depth being bounded by its length.

use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{Mode, lexer::lex};
use ruff_text_size::{TextLen, TextRange};

use crate::parse::MAX_NESTING_DEPTH;

/// Whether `source` can be handed to ruff (or ty) without the parser recursing
/// past [`MAX_NESTING_DEPTH`]; a source of at most `source_scan_threshold`
/// bytes always can.
///
/// Compilation applies the same test and raises `SyntaxError: Source is too
/// deeply nested`, so hosts that also type-check use this to skip the type
/// checker and let the compiler report the located error.
#[must_use]
pub fn source_within_nesting_bound(source: &str, source_scan_threshold: usize) -> bool {
    source.len() <= source_scan_threshold || nesting_bound_exceeded(source, Mode::Module, MAX_NESTING_DEPTH).is_none()
}

/// Estimates ruff's peak parser recursion from one lexer pass, returning the
/// range of the token at which the estimate first exceeds `limit`.
///
/// The estimate never undercounts a recursion point (see [`Scanner`]), so a
/// source that passes cannot recurse more than `limit` plus a small constant
/// for operator-precedence climbing. String literals are scanned in turn,
/// since the type checker parses a forward-reference annotation's contents.
pub(crate) fn nesting_bound_exceeded(source: &str, mode: Mode, limit: u16) -> Option<TextRange> {
    nesting_bound_exceeded_within(source, mode, u32::from(limit), 0)
}

/// How many string literals deep the scan follows before rejecting outright;
/// a forward reference rarely quotes even one level inside another.
const MAX_STRING_LITERAL_DEPTH: u8 = 8;

/// [`nesting_bound_exceeded`] for the contents of a string literal
/// `string_depth` literals deep.
fn nesting_bound_exceeded_within(source: &str, mode: Mode, limit: u32, string_depth: u8) -> Option<TextRange> {
    // Every frame [`Scanner`] charges costs at least one byte of source, so a
    // source no longer than the limit cannot exceed it; that spares nearly
    // every string literal a second lexer.
    if source.len() <= limit as usize {
        return None;
    }
    if string_depth > MAX_STRING_LITERAL_DEPTH {
        return Some(TextRange::up_to(source.text_len()));
    }
    let mut lexer = lex(source, mode);
    let mut scanner = Scanner::new(limit);
    loop {
        let kind = lexer.next_token();
        let range = lexer.current_range();
        if kind == TokenKind::EndOfFile {
            return None;
        }
        // ruff parses an annotation from the raw source between the quotes, so
        // that slice gets the same estimate as the code around it.
        if kind == TokenKind::String
            && let Some((contents, mode)) = string_literal_contents(&source[range])
            && nesting_bound_exceeded_within(contents, mode, limit, string_depth + 1).is_some()
        {
            return Some(range);
        }
        if scanner.observe(kind) {
            return Some(range);
        }
    }
}

/// The source between a string literal's quotes and the mode ruff parses an
/// annotation in that form with; `None` for bytes literals, which are never
/// annotations, and for anything the lexer left unterminated.
fn string_literal_contents(literal: &str) -> Option<(&str, Mode)> {
    let quote_start = literal.find(['\'', '"'])?;
    if literal[..quote_start].contains(['b', 'B']) {
        return None;
    }
    let quoted = &literal[quote_start..];
    let quote = &quoted[..1];
    let triple = if quote == "'" { "'''" } else { "\"\"\"" };
    let (closer, mode) = if quoted.starts_with(triple) {
        (triple, Mode::ParenthesizedExpression)
    } else {
        (quote, Mode::Expression)
    };
    let inner = quoted.get(closer.len()..quoted.len().checked_sub(closer.len())?)?;
    quoted.ends_with(closer).then_some((inner, mode))
}

/// Token-by-token model of the frames ruff's parser holds open.
///
/// ruff recurses on every bracketed expression, prefix operator, `**` right
/// operand, lambda body, conditional `else` branch, f-string format spec,
/// indented block and `case` complex-literal pattern. Each is charged when its
/// token arrives and released when a token proves the frame has returned;
/// anything ambiguous stays charged, so the model only ever overcounts.
struct Scanner {
    limit: u32,
    /// Open bracket levels, bottom entry being the module or expression itself.
    levels: Vec<Level>,
    /// Open indented blocks.
    indent: u32,
    /// Whether the previous token can end an operand, making a following
    /// `-`, `+`, `*` or `not` binary rather than prefix.
    prev_ends_operand: bool,
    /// Whether a `case` began the current logical line, so `+` and `-` join
    /// complex-literal patterns, which recurse on their right operand.
    in_pattern: bool,
    /// Open levels (excluding the bottom), indentation and every level's counts.
    total: u32,
}

/// Frames open at one bracket level.
struct Level {
    kind: LevelKind,
    /// Prefix operators and `**` right operands: any lower-precedence binary
    /// operator proves they have returned.
    tight: u32,
    /// Lambda bodies, `else` branches and format specs: only the level's end
    /// or a comma, newline or statement colon proves they have returned.
    loose: u32,
    /// `lambda` keywords whose `:` has not arrived, so commas are still
    /// separating parameters rather than ending a body.
    pending_lambdas: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelKind {
    /// The module or expression itself.
    Top,
    /// `(`, `[` or `{` outside an f-string.
    Bracket,
    /// Between `FStringStart` and `FStringEnd` (or the t-string pair).
    FString,
    /// A `{...}` replacement field; `in_spec` once its format spec began,
    /// where a nested `{` opens another replacement field.
    Interpolation { in_spec: bool },
}

impl Scanner {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            levels: vec![Level::new(LevelKind::Top)],
            indent: 0,
            prev_ends_operand: false,
            in_pattern: false,
            total: 0,
        }
    }

    /// Accounts for one token, returning whether the estimate now exceeds the limit.
    fn observe(&mut self, kind: TokenKind) -> bool {
        let exceeded = match kind {
            TokenKind::Lpar | TokenKind::Lsqb => self.push(LevelKind::Bracket),
            TokenKind::Lbrace => {
                let kind = match self.top().kind {
                    LevelKind::FString | LevelKind::Interpolation { in_spec: true } => {
                        LevelKind::Interpolation { in_spec: false }
                    }
                    LevelKind::Top | LevelKind::Bracket | LevelKind::Interpolation { in_spec: false } => {
                        LevelKind::Bracket
                    }
                };
                self.push(kind)
            }
            TokenKind::FStringStart | TokenKind::TStringStart => self.push(LevelKind::FString),
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace | TokenKind::FStringEnd | TokenKind::TStringEnd => {
                self.pop();
                false
            }
            TokenKind::Indent => {
                self.indent += 1;
                self.add(1)
            }
            TokenKind::Dedent => {
                if self.indent > 0 {
                    self.indent -= 1;
                    self.total -= 1;
                }
                false
            }
            // `case 1+1+1:` parses each `+` or `-` as a nested complex literal.
            TokenKind::Minus | TokenKind::Plus if self.in_pattern => self.add_tight(1),
            // Binary after an operand (`a - b`, `a * b`, `a not in b`), prefix otherwise.
            TokenKind::Minus | TokenKind::Plus | TokenKind::Star | TokenKind::Not => {
                if self.prev_ends_operand {
                    self.reset_tight();
                    false
                } else {
                    self.add_tight(1)
                }
            }
            // `**` as a binary operator recurses for its right operand; as
            // unpacking it is merely overcounted.
            TokenKind::Tilde | TokenKind::Await | TokenKind::Yield | TokenKind::Async | TokenKind::DoubleStar => {
                self.add_tight(1)
            }
            TokenKind::Lambda => {
                self.top().pending_lambdas += 1;
                false
            }
            // Also lexed for a variable named `case`, where the charge merely overcounts.
            TokenKind::Case => {
                self.in_pattern = true;
                false
            }
            TokenKind::Colon => self.observe_colon(),
            // The `orelse` branch recurses once itself and once for its expression.
            TokenKind::Else => self.add_loose(2),
            // A comma between `lambda` and its `:` separates parameters.
            TokenKind::Comma if self.top().pending_lambdas > 0 => false,
            TokenKind::Comma => {
                self.reset_level();
                false
            }
            TokenKind::Newline | TokenKind::Semi => {
                self.in_pattern = false;
                self.reset_level();
                false
            }
            // Lower precedence than every prefix operator and `**`, so those frames have returned.
            TokenKind::Slash
            | TokenKind::DoubleSlash
            | TokenKind::Percent
            | TokenKind::At
            | TokenKind::Vbar
            | TokenKind::Amper
            | TokenKind::CircumFlex
            | TokenKind::LeftShift
            | TokenKind::RightShift
            | TokenKind::Less
            | TokenKind::Greater
            | TokenKind::EqEqual
            | TokenKind::NotEqual
            | TokenKind::LessEqual
            | TokenKind::GreaterEqual
            | TokenKind::In
            | TokenKind::Is
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::If
            | TokenKind::For => {
                self.reset_tight();
                false
            }
            _ => false,
        };
        self.prev_ends_operand = matches!(
            kind,
            TokenKind::Name
                | TokenKind::Int
                | TokenKind::Float
                | TokenKind::Complex
                | TokenKind::String
                | TokenKind::FStringEnd
                | TokenKind::TStringEnd
                | TokenKind::Rpar
                | TokenKind::Rsqb
                | TokenKind::Rbrace
                | TokenKind::True
                | TokenKind::False
                | TokenKind::None
                | TokenKind::Ellipsis
        );
        exceeded
    }

    /// A lambda's `:` opens its body; any other colon ends the expression
    /// before it, and inside a replacement field opens the format spec.
    fn observe_colon(&mut self) -> bool {
        let top = self.top();
        if top.pending_lambdas > 0 {
            top.pending_lambdas -= 1;
            // The body recurses once itself and once for its expression.
            self.add_loose(2)
        } else {
            self.reset_level();
            let top = self.top();
            if let LevelKind::Interpolation { in_spec } = &mut top.kind {
                *in_spec = true;
                self.add_loose(1)
            } else {
                false
            }
        }
    }

    fn top(&mut self) -> &mut Level {
        self.levels.last_mut().expect("the bottom level is never popped")
    }

    fn push(&mut self, kind: LevelKind) -> bool {
        self.levels.push(Level::new(kind));
        self.add(1)
    }

    fn pop(&mut self) {
        if self.levels.len() > 1 {
            let level = self.levels.pop().expect("length checked above");
            self.total -= 1 + level.tight + level.loose;
        }
    }

    fn add_tight(&mut self, frames: u32) -> bool {
        self.top().tight += frames;
        self.add(frames)
    }

    fn add_loose(&mut self, frames: u32) -> bool {
        self.top().loose += frames;
        self.add(frames)
    }

    fn reset_tight(&mut self) {
        let top = self.top();
        let released = top.tight;
        top.tight = 0;
        self.total -= released;
    }

    fn reset_level(&mut self) {
        let top = self.top();
        let released = top.tight + top.loose;
        top.tight = 0;
        top.loose = 0;
        self.total -= released;
    }

    fn add(&mut self, frames: u32) -> bool {
        self.total += frames;
        self.total > self.limit
    }
}

impl Level {
    fn new(kind: LevelKind) -> Self {
        Self {
            kind,
            tight: 0,
            loose: 0,
            pending_lambdas: 0,
        }
    }
}
