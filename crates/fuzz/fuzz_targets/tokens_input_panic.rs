//! Fuzz target using structured token input instead of random strings.
//!
//! This generates more syntactically plausible Python code by combining
//! tokens that represent common Python constructs. The fuzzer explores
//! combinations of these tokens to find edge cases.
#![no_main]

use std::{
    fmt::{self, Display},
    time::Duration,
};

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use monty::{LimitedTracker, MontyRun, NoPrint, ResourceLimits};

/// A token representing a piece of Python syntax.
#[derive(Debug, Clone, Arbitrary)]
enum Token {
    // === Literals ===
    String(StringLit),
    Int(i64),
    Float(FloatLit),
    Bool(bool),
    None,

    // === Identifiers ===
    Var(VarName),
    Attr(AttrName),

    // === Operators ===
    BinOp(BinOp),
    UnaryOp(UnaryOp),
    CompareOp(CompareOp),
    AugAssign(AugAssign),

    // === Keywords ===
    Keyword(Keyword),

    // === Punctuation ===
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Dot,
    Arrow,
    Assign,
    Walrus,

    // === Whitespace/Structure ===
    Space,
    Newline,
    Indent(IndentLevel),
    Comment,
}

/// String literal variants.
#[derive(Debug, Clone, Arbitrary)]
enum StringLit {
    Empty,
    Short(ShortString),
    FString(ShortString),
    Raw(ShortString),
    Bytes(ShortString),
}

/// Short string content (limited to avoid huge inputs).
#[derive(Debug, Clone, Arbitrary)]
enum ShortString {
    Hello,
    World,
    Test,
    Foo,
    Bar,
    Empty,
    Space,
    Newline,
    Number,
    Special,
}

/// Float literal (avoiding infinity/NaN issues).
#[derive(Debug, Clone, Arbitrary)]
enum FloatLit {
    Zero,
    One,
    Half,
    Pi,
    Negative,
    Small,
    Large,
}

/// Common variable names.
#[derive(Debug, Clone, Arbitrary)]
enum VarName {
    X,
    Y,
    Z,
    A,
    B,
    C,
    I,
    J,
    N,
    Foo,
    Bar,
    Baz,
    Spam,
    Eggs,
    Result,
    Value,
    Item,
    Data,
    Args,
    Kwargs,
    Self_,
    Cls,
}

/// Common attribute names.
#[derive(Debug, Clone, Arbitrary)]
enum AttrName {
    Append,
    Pop,
    Get,
    Set,
    Keys,
    Values,
    Items,
    Join,
    Split,
    Strip,
    Lower,
    Upper,
    Format,
    Replace,
    Find,
    Count,
    Sort,
    Reverse,
    Copy,
    Clear,
    Update,
    Add,
    Remove,
}

/// Binary operators.
#[derive(Debug, Clone, Arbitrary)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    MatMul,
    BitAnd,
    BitOr,
    BitXor,
    LShift,
    RShift,
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Arbitrary)]
enum UnaryOp {
    Neg,
    Pos,
    Not,
    Invert,
}

/// Comparison operators.
#[derive(Debug, Clone, Arbitrary)]
enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    IsNot,
    In,
    NotIn,
}

/// Augmented assignment operators.
#[derive(Debug, Clone, Arbitrary)]
enum AugAssign {
    AddEq,
    SubEq,
    MulEq,
    DivEq,
    FloorDivEq,
    ModEq,
    PowEq,
    AndEq,
    OrEq,
    XorEq,
    LShiftEq,
    RShiftEq,
}

/// Python keywords.
#[derive(Debug, Clone, Arbitrary)]
enum Keyword {
    If,
    Elif,
    Else,
    For,
    While,
    Break,
    Continue,
    Pass,
    Return,
    Def,
    Lambda,
    Async,
    Await,
    Try,
    Except,
    Finally,
    Raise,
    Assert,
    Import,
    From,
    As,
    Global,
    Nonlocal,
}

/// Indentation levels (0-4 levels deep).
#[derive(Debug, Clone, Arbitrary)]
enum IndentLevel {
    L0,
    L1,
    L2,
    L3,
    L4,
}

impl Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{s}"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Float(fl) => write!(f, "{fl}"),
            Self::Bool(true) => write!(f, "True"),
            Self::Bool(false) => write!(f, "False"),
            Self::None => write!(f, "None"),
            Self::Var(v) => write!(f, "{v}"),
            Self::Attr(a) => write!(f, "{a}"),
            Self::BinOp(op) => write!(f, "{op}"),
            Self::UnaryOp(op) => write!(f, "{op}"),
            Self::CompareOp(op) => write!(f, "{op}"),
            Self::AugAssign(op) => write!(f, "{op}"),
            Self::Keyword(kw) => write!(f, "{kw}"),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::LBracket => write!(f, "["),
            Self::RBracket => write!(f, "]"),
            Self::LBrace => write!(f, "{{"),
            Self::RBrace => write!(f, "}}"),
            Self::Comma => write!(f, ","),
            Self::Colon => write!(f, ":"),
            Self::Semicolon => write!(f, ";"),
            Self::Dot => write!(f, "."),
            Self::Arrow => write!(f, "->"),
            Self::Assign => write!(f, "="),
            Self::Walrus => write!(f, ":="),
            Self::Space => write!(f, " "),
            Self::Newline => writeln!(f),
            Self::Indent(level) => write!(f, "{level}"),
            Self::Comment => write!(f, "# comment"),
        }
    }
}

impl Display for StringLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "''"),
            Self::Short(s) => write!(f, "'{s}'"),
            Self::FString(s) => write!(f, "f'{s}'"),
            Self::Raw(s) => write!(f, "r'{s}'"),
            Self::Bytes(s) => write!(f, "b'{s}'"),
        }
    }
}

impl Display for ShortString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello => write!(f, "hello"),
            Self::World => write!(f, "world"),
            Self::Test => write!(f, "test"),
            Self::Foo => write!(f, "foo"),
            Self::Bar => write!(f, "bar"),
            Self::Empty => write!(f, ""),
            Self::Space => write!(f, " "),
            Self::Newline => write!(f, "\\n"),
            Self::Number => write!(f, "123"),
            Self::Special => write!(f, "{{}}"),
        }
    }
}

impl Display for FloatLit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(f, "0.0"),
            Self::One => write!(f, "1.0"),
            Self::Half => write!(f, "0.5"),
            Self::Pi => write!(f, "3.14159"),
            Self::Negative => write!(f, "-1.5"),
            Self::Small => write!(f, "0.001"),
            Self::Large => write!(f, "1e10"),
        }
    }
}

impl Display for VarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X => write!(f, "x"),
            Self::Y => write!(f, "y"),
            Self::Z => write!(f, "z"),
            Self::A => write!(f, "a"),
            Self::B => write!(f, "b"),
            Self::C => write!(f, "c"),
            Self::I => write!(f, "i"),
            Self::J => write!(f, "j"),
            Self::N => write!(f, "n"),
            Self::Foo => write!(f, "foo"),
            Self::Bar => write!(f, "bar"),
            Self::Baz => write!(f, "baz"),
            Self::Spam => write!(f, "spam"),
            Self::Eggs => write!(f, "eggs"),
            Self::Result => write!(f, "result"),
            Self::Value => write!(f, "value"),
            Self::Item => write!(f, "item"),
            Self::Data => write!(f, "data"),
            Self::Args => write!(f, "args"),
            Self::Kwargs => write!(f, "kwargs"),
            Self::Self_ => write!(f, "self"),
            Self::Cls => write!(f, "cls"),
        }
    }
}

impl Display for AttrName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Append => write!(f, "append"),
            Self::Pop => write!(f, "pop"),
            Self::Get => write!(f, "get"),
            Self::Set => write!(f, "set"),
            Self::Keys => write!(f, "keys"),
            Self::Values => write!(f, "values"),
            Self::Items => write!(f, "items"),
            Self::Join => write!(f, "join"),
            Self::Split => write!(f, "split"),
            Self::Strip => write!(f, "strip"),
            Self::Lower => write!(f, "lower"),
            Self::Upper => write!(f, "upper"),
            Self::Format => write!(f, "format"),
            Self::Replace => write!(f, "replace"),
            Self::Find => write!(f, "find"),
            Self::Count => write!(f, "count"),
            Self::Sort => write!(f, "sort"),
            Self::Reverse => write!(f, "reverse"),
            Self::Copy => write!(f, "copy"),
            Self::Clear => write!(f, "clear"),
            Self::Update => write!(f, "update"),
            Self::Add => write!(f, "add"),
            Self::Remove => write!(f, "remove"),
        }
    }
}

impl Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Add => write!(f, " + "),
            Self::Sub => write!(f, " - "),
            Self::Mul => write!(f, " * "),
            Self::Div => write!(f, " / "),
            Self::FloorDiv => write!(f, " // "),
            Self::Mod => write!(f, " % "),
            Self::Pow => write!(f, " ** "),
            Self::MatMul => write!(f, " @ "),
            Self::BitAnd => write!(f, " & "),
            Self::BitOr => write!(f, " | "),
            Self::BitXor => write!(f, " ^ "),
            Self::LShift => write!(f, " << "),
            Self::RShift => write!(f, " >> "),
            Self::And => write!(f, " and "),
            Self::Or => write!(f, " or "),
        }
    }
}

impl Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Neg => write!(f, "-"),
            Self::Pos => write!(f, "+"),
            Self::Not => write!(f, "not "),
            Self::Invert => write!(f, "~"),
        }
    }
}

impl Display for CompareOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eq => write!(f, " == "),
            Self::Ne => write!(f, " != "),
            Self::Lt => write!(f, " < "),
            Self::Le => write!(f, " <= "),
            Self::Gt => write!(f, " > "),
            Self::Ge => write!(f, " >= "),
            Self::Is => write!(f, " is "),
            Self::IsNot => write!(f, " is not "),
            Self::In => write!(f, " in "),
            Self::NotIn => write!(f, " not in "),
        }
    }
}

impl Display for AugAssign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddEq => write!(f, " += "),
            Self::SubEq => write!(f, " -= "),
            Self::MulEq => write!(f, " *= "),
            Self::DivEq => write!(f, " /= "),
            Self::FloorDivEq => write!(f, " //= "),
            Self::ModEq => write!(f, " %= "),
            Self::PowEq => write!(f, " **= "),
            Self::AndEq => write!(f, " &= "),
            Self::OrEq => write!(f, " |= "),
            Self::XorEq => write!(f, " ^= "),
            Self::LShiftEq => write!(f, " <<= "),
            Self::RShiftEq => write!(f, " >>= "),
        }
    }
}

impl Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::If => write!(f, "if "),
            Self::Elif => write!(f, "elif "),
            Self::Else => write!(f, "else"),
            Self::For => write!(f, "for "),
            Self::While => write!(f, "while "),
            Self::Break => write!(f, "break"),
            Self::Continue => write!(f, "continue"),
            Self::Pass => write!(f, "pass"),
            Self::Return => write!(f, "return "),
            Self::Def => write!(f, "def "),
            Self::Lambda => write!(f, "lambda "),
            Self::Async => write!(f, "async "),
            Self::Await => write!(f, "await "),
            Self::Try => write!(f, "try"),
            Self::Except => write!(f, "except "),
            Self::Finally => write!(f, "finally"),
            Self::Raise => write!(f, "raise "),
            Self::Assert => write!(f, "assert "),
            Self::Import => write!(f, "import "),
            Self::From => write!(f, "from "),
            Self::As => write!(f, " as "),
            Self::Global => write!(f, "global "),
            Self::Nonlocal => write!(f, "nonlocal "),
        }
    }
}

impl Display for IndentLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spaces = match self {
            Self::L0 => 0,
            Self::L1 => 4,
            Self::L2 => 8,
            Self::L3 => 12,
            Self::L4 => 16,
        };
        for _ in 0..spaces {
            write!(f, " ")?;
        }
        Ok(())
    }
}

/// Wrapper for `Vec<Token>` with custom Debug that shows both tokens and generated code.
struct Tokens(Vec<Token>);

impl<'a> Arbitrary<'a> for Tokens {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Vec::<Token>::arbitrary(u).map(Tokens)
    }
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("tokens", &self.0)
            .field("code", &self.to_code())
            .finish()
    }
}

impl Tokens {
    /// Convert the tokens to Python source code.
    fn to_code(&self) -> String {
        self.0.iter().map(|t| t.to_string()).collect()
    }
}

/// Resource limits for fuzzing.
fn fuzz_limits() -> LimitedTracker {
    LimitedTracker::new(
        ResourceLimits::new()
            .max_allocations(10_000)
            .max_memory(1024 * 1024) // 1 MB
            .max_duration(Duration::from_millis(100)),
    )
}

fuzz_target!(|tokens: Tokens| {
    let code = tokens.to_code();

    // Try to parse the code
    let Ok(runner) = MontyRun::new(code, "fuzz.py", vec![], vec![]) else {
        return; // Parse errors are expected
    };

    // Try to execute with resource limits
    let _ = runner.run(vec![], fuzz_limits(), &mut NoPrint);
});
