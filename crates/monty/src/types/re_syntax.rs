//! Translation of CPython `re` pattern syntax into `fancy-regex` syntax.
//!
//! `fancy-regex` accepts Oniguruma and PCRE extensions that CPython rejects, among them subroutine calls,
//! whose compile-time expansion is unbounded. It also reads some shared syntax differently: `\<` and `\Z`,
//! octal escapes, and set operators inside classes.
//! [`translate`] parses a pattern the way CPython's `re._parser` does, raising its errors with the same
//! messages and positions, and emits an equivalent pattern built only from syntax both engines agree on.
//! Literal punctuation is emitted as `\x{..}`, so nothing in the output can be read as an extension.

use std::{
    collections::HashMap,
    fmt::{self, Write},
};

use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    modules::re::{ASCII, DOTALL, IGNORECASE, MULTILINE},
    types::str::{StringRepr, str_isidentifier},
};

/// `re.LOCALE`, which CPython rejects for str patterns.
const LOCALE: u16 = 4;
/// `re.UNICODE`, incompatible with `re.ASCII`.
const UNICODE: u16 = 32;
/// `re.VERBOSE`: whitespace and `#` comments outside classes are ignored.
const VERBOSE: u16 = 64;
/// The flags that pick how character classes behave; at most one may be set.
const TYPE_FLAGS: u16 = ASCII | LOCALE | UNICODE;

/// CPython's `MAXREPEAT`: repeat counts must be below it, and it stands for "unbounded".
const MAXREPEAT: u64 = 4_294_967_295;
/// CPython's `MAXGROUPS`; a conditional naming a group at or above it is invalid.
const MAXGROUPS: u64 = 1_073_741_823;
/// CPython's `MAXCODE`: the furthest a lookbehind may look.
const MAXCODE: u128 = 4_294_967_295;
/// CPython's `MAXWIDTH`, the saturation point of subpattern widths.
const MAXWIDTH: u128 = 1 << 64;
/// Group nesting at which `fancy-regex` gives up; checked here first so the recursion stays bounded.
const MAX_NESTING: usize = 64;

/// Translates a CPython regex `pattern` into `fancy-regex` syntax, validating it as CPython does.
///
/// `flags` are the `flags=` argument; they are not applied to the output, only checked for consistency
/// with inline flags (the caller prefixes the flag groups itself).
///
/// # Errors
///
/// `re.PatternError` with CPython's message for an invalid pattern, `OverflowError` for a repeat count
/// that is too large and `ValueError` for incompatible flags.
pub(crate) fn translate(pattern: &str, flags: u16) -> RunResult<String> {
    let mut translator = Translator::new(pattern, flags)?;
    translator.parse_sub(flags & VERBOSE != 0, 0)?;
    translator.finish()
}

/// One token of the pattern: a character, or a backslash and the character after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Char(char),
    Escape(char),
}

impl Token {
    /// Length of the token in pattern characters, which is what CPython's error positions count.
    fn len(self) -> usize {
        match self {
            Self::Char(_) => 1,
            Self::Escape(_) => 2,
        }
    }

    /// The character after the backslash, or the character itself.
    fn char(self) -> char {
        match self {
            Self::Char(c) | Self::Escape(c) => c,
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(c) => write!(f, "{c}"),
            Self::Escape(c) => write!(f, "\\{c}"),
        }
    }
}

/// A port of CPython's `re._parser.Tokenizer`: one token of lookahead over the pattern's characters.
struct Tokenizer {
    chars: Vec<char>,
    /// Index just past `next`.
    index: usize,
    next: Option<Token>,
}

impl Tokenizer {
    fn new(pattern: &str) -> RunResult<Self> {
        let mut tokenizer = Self {
            chars: pattern.chars().collect(),
            index: 0,
            next: None,
        };
        tokenizer.advance()?;
        Ok(tokenizer)
    }

    /// Reads the token at `index` into `next`.
    fn advance(&mut self) -> RunResult<()> {
        self.next = match self.chars.get(self.index) {
            None => None,
            Some('\\') => match self.chars.get(self.index + 1) {
                Some(&c) => Some(Token::Escape(c)),
                None => {
                    return Err(self.error_at("bad escape (end of pattern)", self.chars.len() - 1));
                }
            },
            Some(&c) => Some(Token::Char(c)),
        };
        self.index += self.next.map_or(0, Token::len);
        Ok(())
    }

    /// Consumes and returns the next token.
    fn get(&mut self) -> RunResult<Option<Token>> {
        let this = self.next;
        self.advance()?;
        Ok(this)
    }

    /// Consumes the next token if it is the plain character `c`.
    fn matches(&mut self, c: char) -> RunResult<bool> {
        if self.next == Some(Token::Char(c)) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// The next token if it is a plain character.
    fn next_char(&self) -> Option<char> {
        match self.next {
            Some(Token::Char(c)) => Some(c),
            _ => None,
        }
    }

    /// Position of the next token.
    fn tell(&self) -> usize {
        self.index - self.next.map_or(0, Token::len)
    }

    fn seek(&mut self, index: usize) -> RunResult<()> {
        self.index = index;
        self.advance()
    }

    /// Consumes up to `n` plain characters matching `accept`.
    fn get_while(&mut self, n: usize, accept: impl Fn(char) -> bool) -> RunResult<String> {
        let mut result = String::new();
        for _ in 0..n {
            match self.next_char() {
                Some(c) if accept(c) => {
                    result.push(c);
                    self.advance()?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    /// Consumes tokens up to and including `terminator`, returning the text before it.
    ///
    /// `name` describes the text for the "missing ..." errors.
    fn get_until(&mut self, terminator: char, name: &str) -> RunResult<String> {
        let mut result = String::new();
        let mut len = 0;
        loop {
            match self.get()? {
                None if result.is_empty() => return Err(self.error(format!("missing {name}"), 0)),
                None => return Err(self.error(format!("missing {terminator}, unterminated name"), len)),
                Some(Token::Char(c)) if c == terminator => {
                    return if result.is_empty() {
                        Err(self.error(format!("missing {name}"), 1))
                    } else {
                        Ok(result)
                    };
                }
                Some(token) => {
                    result.push_str(&token.to_string());
                    len += token.len();
                }
            }
        }
    }

    /// Rejects a group name that is not an identifier.
    fn check_group_name(&self, name: &str, offset: usize) -> RunResult<()> {
        if str_isidentifier(name) {
            Ok(())
        } else {
            let msg = format!("bad character in group name {}", StringRepr(name));
            Err(self.error(msg, name.chars().count() + offset))
        }
    }

    /// A `re.PatternError` at `offset` characters before the next token.
    fn error(&self, msg: impl fmt::Display, offset: usize) -> RunError {
        self.error_at(msg, self.tell().saturating_sub(offset))
    }

    /// A `re.PatternError` at `pos`, formatted as CPython's `PatternError.__init__` does.
    fn error_at(&self, msg: impl fmt::Display, pos: usize) -> RunError {
        let before = &self.chars[..pos.min(self.chars.len())];
        let msg = if self.chars.contains(&'\n') {
            let line = before.iter().filter(|&&c| c == '\n').count() + 1;
            let column = before.iter().rposition(|&c| c == '\n').map_or(pos + 1, |i| pos - i);
            format!("{msg} at position {pos} (line {line}, column {column})")
        } else {
            format!("{msg} at position {pos}")
        };
        ExcType::re_pattern_error(msg)
    }
}

/// Smallest and largest number of characters a subpattern can match, saturating at [`MAXWIDTH`].
type Width = (u128, u128);

/// What CPython's repeat operators need to know about the item before them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    /// An anchor (`^`, `$`, `\A`, `\b`, ...): nothing to repeat.
    At,
    /// Already repeated: another quantifier is a "multiple repeat".
    Repeat,
    Other,
}

/// One parsed element of a sequence.
#[derive(Debug, Clone, Copy)]
struct Item {
    kind: ItemKind,
    width: Width,
}

impl Item {
    /// An item matching exactly one character.
    const UNIT: Self = Self {
        kind: ItemKind::Other,
        width: (1, 1),
    };
    /// A zero-width anchor.
    const AT: Self = Self {
        kind: ItemKind::At,
        width: (0, 0),
    };
    /// A zero-width item that is not an anchor (a lookaround).
    const EMPTY: Self = Self {
        kind: ItemKind::Other,
        width: (0, 0),
    };
}

/// What a `(` construct contributed to its sequence.
enum GroupOutcome {
    Item(Item),
    /// A `(?#...)` comment.
    Nothing,
    /// Global flags such as `(?x)`, which may change verbosity.
    GlobalFlags,
}

/// A member of a character class.
#[derive(Debug, Clone, Copy)]
enum ClassItem {
    /// A code point, which may be a surrogate no Monty string contains.
    Literal(u32),
    Range(u32, u32),
    /// `\d`, `\D`, `\s`, `\S`, `\w` or `\W`.
    Category(char),
}

/// Parser state, a port of CPython's `re._parser.State` plus the output being built.
struct Translator {
    source: Tokenizer,
    out: String,
    /// The `flags=` argument plus any global inline flags.
    flags: u16,
    /// Widths of closed groups by number; `None` while open. Index 0 stands for the whole pattern.
    group_widths: Vec<Option<Width>>,
    group_names: HashMap<String, usize>,
    /// Number of groups when the outermost enclosing lookbehind opened.
    lookbehind_groups: Option<usize>,
    /// Groups named by numbered conditionals and where, in order, repeats included; checked once all groups are known.
    groupref_pos: Vec<(u64, usize)>,
    /// The first lookbehind CPython's compiler would reject, by start position, and its message.
    lookbehind_error: Option<(usize, &'static str)>,
    depth: usize,
}

impl Translator {
    fn new(pattern: &str, flags: u16) -> RunResult<Self> {
        Ok(Self {
            source: Tokenizer::new(pattern)?,
            out: String::with_capacity(pattern.len()),
            flags,
            group_widths: vec![None],
            group_names: HashMap::new(),
            lookbehind_groups: None,
            groupref_pos: Vec::new(),
            lookbehind_error: None,
            depth: 0,
        })
    }

    /// The checks CPython makes after parsing, in its order; returns the translated pattern.
    fn finish(self) -> RunResult<String> {
        if self.flags & LOCALE != 0 {
            return Err(ExcType::value_error("cannot use LOCALE flag with a str pattern"));
        }
        if self.flags & ASCII != 0 && self.flags & UNICODE != 0 {
            return Err(ExcType::value_error("ASCII and UNICODE flags are incompatible"));
        }
        if self.source.next.is_some() {
            return Err(self.source.error("unbalanced parenthesis", 0));
        }
        let groups = self.group_widths.len() as u64;
        if let Some(&(group, pos)) = self.groupref_pos.iter().find(|(group, _)| *group >= groups) {
            return Err(self.source.error_at(format!("invalid group reference {group}"), pos));
        }
        match self.lookbehind_error {
            Some((_, msg)) => Err(ExcType::re_pattern_error(msg)),
            None => Ok(self.out),
        }
    }

    /// Parses an alternation, port of `_parse_sub`.
    fn parse_sub(&mut self, mut verbose: bool, nested: usize) -> RunResult<Width> {
        let mut width = (MAXWIDTH, 0);
        let mut first = nested == 0;
        loop {
            let branch = self.parse(verbose, nested + 1, first)?;
            width = (width.0.min(branch.0), width.1.max(branch.1));
            if !self.source.matches('|')? {
                return Ok(width);
            }
            self.out.push('|');
            first = false;
            if nested == 0 {
                verbose = self.flags & VERBOSE != 0;
            }
        }
    }

    /// Parses a sequence up to `|`, `)` or the end, port of `_parse`.
    ///
    /// `first` is true only for the first top-level branch, where global flags are allowed.
    fn parse(&mut self, mut verbose: bool, nested: usize, first: bool) -> RunResult<Width> {
        let mut items: Vec<Item> = Vec::new();
        while let Some(this) = self.source.next {
            if matches!(this, Token::Char('|' | ')')) {
                break;
            }
            self.source.advance()?;
            if verbose {
                match this {
                    Token::Char(' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') => continue,
                    Token::Char('#') => {
                        while !matches!(self.source.get()?, None | Some(Token::Char('\n'))) {}
                        continue;
                    }
                    _ => {}
                }
            }
            match this {
                Token::Escape(c) => items.push(self.escape(c)?),
                Token::Char('[') => {
                    self.class()?;
                    items.push(Item::UNIT);
                }
                Token::Char(c @ ('*' | '+' | '?' | '{')) => self.repeat(c, &mut items)?,
                Token::Char('.') => {
                    self.out.push('.');
                    items.push(Item::UNIT);
                }
                Token::Char('(') => match self.group(verbose, nested, first && items.is_empty())? {
                    GroupOutcome::Item(item) => items.push(item),
                    GroupOutcome::Nothing => {}
                    GroupOutcome::GlobalFlags => verbose = self.flags & VERBOSE != 0,
                },
                Token::Char(c @ ('^' | '$')) => {
                    self.out.push(c);
                    items.push(Item::AT);
                }
                Token::Char(c) => {
                    push_literal(&mut self.out, u32::from(c));
                    items.push(Item::UNIT);
                }
            }
        }
        Ok(items.iter().fold((0, 0), |acc, item| add_widths(acc, item.width)))
    }

    /// Handles an escape outside a class, port of `_escape`.
    fn escape(&mut self, c: char) -> RunResult<Item> {
        Ok(match c {
            'A' => self.push_at("\\A"),
            'b' => self.push_at("\\b"),
            'B' => self.push_at("\\B"),
            // CPython's `\Z` is the end of the string; `fancy-regex`'s also allows trailing newlines.
            'z' | 'Z' => self.push_at("\\z"),
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => {
                self.out.push('\\');
                self.out.push(c);
                Item::UNIT
            }
            '1'..='9' => return self.numbered_escape(c),
            _ => {
                let cp = match self.literal_escape(c, false)? {
                    Some(cp) => cp,
                    None if c == '0' => {
                        let digits = self.source.get_while(2, |d| d.is_digit(8))?;
                        u32::from_str_radix(&digits, 8).unwrap_or(0)
                    }
                    None if c.is_ascii_alphabetic() => return Err(self.source.error(format!("bad escape \\{c}"), 2)),
                    None => u32::from(c),
                };
                push_literal(&mut self.out, cp);
                Item::UNIT
            }
        })
    }

    /// `\1`-`\99`, a group reference, or a three-digit octal escape.
    fn numbered_escape(&mut self, first: char) -> RunResult<Item> {
        let mut digits = String::from(first);
        if let Some(second) = self.source.next_char().filter(char::is_ascii_digit) {
            self.source.advance()?;
            digits.push(second);
            let third = self.source.next_char().filter(|d| d.is_digit(8));
            if let Some(third) = third.filter(|_| first.is_digit(8) && second.is_digit(8)) {
                self.source.advance()?;
                digits.push(third);
                let cp = u32::from_str_radix(&digits, 8).expect("three octal digits");
                if cp > 0o377 {
                    let msg = format!("octal escape value \\{digits} outside of range 0-0o377");
                    return Err(self.source.error(msg, digits.len() + 1));
                }
                push_literal(&mut self.out, cp);
                return Ok(Item::UNIT);
            }
        }
        let group: usize = digits.parse().expect("one or two digits");
        if group >= self.group_widths.len() {
            return Err(self
                .source
                .error(format!("invalid group reference {group}"), digits.len()));
        }
        if self.group_widths[group].is_none() {
            return Err(self.source.error("cannot refer to an open group", digits.len() + 1));
        }
        self.check_lookbehind_group(group)?;
        Ok(self.push_backref(group))
    }

    /// The escapes that mean the same character inside and outside a class.
    ///
    /// Returns `None` for an escape the caller handles itself.
    fn literal_escape(&mut self, c: char, in_class: bool) -> RunResult<Option<u32>> {
        Ok(Some(match c {
            'a' => 0x07,
            'b' if in_class => 0x08,
            'f' => 0x0c,
            'n' => u32::from('\n'),
            'r' => u32::from('\r'),
            't' => u32::from('\t'),
            'v' => 0x0b,
            '\\' => u32::from('\\'),
            'x' => self.hex_escape(c, 2)?,
            'u' => self.hex_escape(c, 4)?,
            'U' => self.hex_escape(c, 8)?,
            'N' => self.named_escape()?,
            _ => return Ok(None),
        }))
    }

    /// `\xhh`, `\uhhhh` or `\Uhhhhhhhh`: exactly `digits` hex digits.
    fn hex_escape(&mut self, c: char, digits: usize) -> RunResult<u32> {
        let hex = self.source.get_while(digits, |d| d.is_ascii_hexdigit())?;
        let escape = format!("\\{c}{hex}");
        if hex.len() != digits {
            return Err(self.source.error(format!("incomplete escape {escape}"), escape.len()));
        }
        match u32::from_str_radix(&hex, 16) {
            Ok(cp) if cp <= 0x10_FFFF => Ok(cp),
            _ => Err(self.source.error(format!("bad escape {escape}"), escape.len())),
        }
    }

    /// `\N{NAME}`, a character by its Unicode name.
    fn named_escape(&mut self) -> RunResult<u32> {
        if !self.source.matches('{')? {
            return Err(self.source.error("missing {", 0));
        }
        let name = self.source.get_until('}', "character name")?;
        if let Some(c) = unicode_names2::character(&name) {
            Ok(u32::from(c))
        } else {
            let msg = format!("undefined character name {}", StringRepr(&name));
            Err(self.source.error(msg, name.chars().count() + 4))
        }
    }

    /// Handles a character class after its `[`, port of the class branch of `_parse`.
    fn class(&mut self) -> RunResult<()> {
        let here = self.source.tell() - 1;
        let negate = self.source.matches('^')?;
        let mut members: Vec<ClassItem> = Vec::new();
        loop {
            let Some(this) = self.source.get()? else {
                return Err(self
                    .source
                    .error("unterminated character set", self.source.tell() - here));
            };
            if this == Token::Char(']') && !members.is_empty() {
                break;
            }
            let first = self.class_member(this)?;
            if !self.source.matches('-')? {
                members.push(first);
                continue;
            }
            let Some(that) = self.source.get()? else {
                return Err(self
                    .source
                    .error("unterminated character set", self.source.tell() - here));
            };
            if that == Token::Char(']') {
                members.push(first);
                members.push(ClassItem::Literal(u32::from('-')));
                break;
            }
            let second = self.class_member(that)?;
            match (first, second) {
                (ClassItem::Literal(lo), ClassItem::Literal(hi)) if lo <= hi => members.push(ClassItem::Range(lo, hi)),
                _ => {
                    let msg = format!("bad character range {this}-{that}");
                    return Err(self.source.error(msg, this.len() + 1 + that.len()));
                }
            }
        }
        push_class(&mut self.out, negate, &members);
        Ok(())
    }

    /// One class member from its first token, port of `_class_escape` for escapes.
    fn class_member(&mut self, token: Token) -> RunResult<ClassItem> {
        let c = match token {
            Token::Char(c) => return Ok(ClassItem::Literal(u32::from(c))),
            Token::Escape(c) => c,
        };
        if matches!(c, 'd' | 'D' | 's' | 'S' | 'w' | 'W') {
            return Ok(ClassItem::Category(c));
        }
        if let Some(cp) = self.literal_escape(c, true)? {
            return Ok(ClassItem::Literal(cp));
        }
        if c.is_digit(8) {
            let digits = format!("{c}{}", self.source.get_while(2, |d| d.is_digit(8))?);
            let cp = u32::from_str_radix(&digits, 8).expect("octal digits");
            return if cp > 0o377 {
                let msg = format!("octal escape value \\{digits} outside of range 0-0o377");
                Err(self.source.error(msg, digits.len() + 1))
            } else {
                Ok(ClassItem::Literal(cp))
            };
        }
        if c.is_ascii_alphanumeric() {
            Err(self.source.error(format!("bad escape \\{c}"), 2))
        } else {
            Ok(ClassItem::Literal(u32::from(c)))
        }
    }

    /// Handles a quantifier, port of the repeat branch of `_parse`.
    fn repeat(&mut self, this: char, items: &mut Vec<Item>) -> RunResult<()> {
        let here = self.source.tell();
        let bounds = match this {
            '?' => Some((0, 1)),
            '*' => Some((0, MAXREPEAT)),
            '+' => Some((1, MAXREPEAT)),
            _ => self.brace_bounds(here)?,
        };
        let Some((min, max)) = bounds else {
            push_literal(&mut self.out, u32::from('{'));
            items.push(Item::UNIT);
            return Ok(());
        };
        let offset = self.source.tell() - here + 1;
        let Some(item) = items.last_mut().filter(|item| item.kind != ItemKind::At) else {
            return Err(self.source.error("nothing to repeat", offset));
        };
        if item.kind == ItemKind::Repeat {
            return Err(self.source.error("multiple repeat", offset));
        }
        self.out.push_str(&quantifier(min, max));
        if self.source.matches('?')? {
            self.out.push('?');
        } else if self.source.matches('+')? {
            self.out.push('+');
        }
        let (lo, hi) = item.width;
        let hi = if max == MAXREPEAT && hi != 0 {
            MAXWIDTH
        } else {
            hi * u128::from(max)
        };
        *item = Item {
            kind: ItemKind::Repeat,
            width: ((lo * u128::from(min)).min(MAXWIDTH), hi.min(MAXWIDTH)),
        };
        Ok(())
    }

    /// Parses `{m,n}` after its `{`; `None` when the brace is a literal, with the position restored.
    fn brace_bounds(&mut self, here: usize) -> RunResult<Option<(u64, u64)>> {
        if self.source.next_char() == Some('}') {
            return Ok(None);
        }
        let lo = self.source.get_while(usize::MAX, |c| c.is_ascii_digit())?;
        let hi = if self.source.matches(',')? {
            self.source.get_while(usize::MAX, |c| c.is_ascii_digit())?
        } else {
            lo.clone()
        };
        if !self.source.matches('}')? {
            self.source.seek(here)?;
            return Ok(None);
        }
        let min = if lo.is_empty() { 0 } else { repeat_count(&lo)? };
        let max = if hi.is_empty() { MAXREPEAT } else { repeat_count(&hi)? };
        if max < min {
            return Err(self
                .source
                .error("min repeat greater than max repeat", self.source.tell() - here));
        }
        Ok(Some((min, max)))
    }

    /// Emits a zero-width anchor.
    fn push_at(&mut self, anchor: &str) -> Item {
        self.out.push_str(anchor);
        Item::AT
    }

    /// Emits a reference to the closed group `group`; it matches what the group matched.
    fn push_backref(&mut self, group: usize) -> Item {
        write!(self.out, "\\k<{group}>").expect("writing to a String is infallible");
        Item {
            kind: ItemKind::Other,
            width: self.group_widths[group].expect("referenced group is closed"),
        }
    }

    /// True when `group` exists and has been closed.
    fn is_closed(&self, group: usize) -> bool {
        self.group_widths.get(group).is_some_and(Option::is_some)
    }

    /// Rejects a reference from inside a lookbehind to an open group or one defined in that lookbehind.
    fn check_lookbehind_group(&self, group: usize) -> RunResult<()> {
        match self.lookbehind_groups {
            Some(_) if !self.is_closed(group) => Err(self.source.error("cannot refer to an open group", 0)),
            Some(first) if group >= first => Err(self
                .source
                .error("cannot refer to group defined in the same lookbehind subpattern", 0)),
            _ => Ok(()),
        }
    }

    /// Numbers a new capturing group, rejecting a repeated name.
    fn open_group(&mut self, name: Option<&str>) -> RunResult<usize> {
        let group = self.group_widths.len();
        self.group_widths.push(None);
        if let Some(name) = name {
            if let Some(&previous) = self.group_names.get(name) {
                let msg = format!(
                    "redefinition of group name {} as group {group}; was group {previous}",
                    StringRepr(name)
                );
                return Err(self.source.error(msg, name.chars().count() + 1));
            }
            self.group_names.insert(name.to_owned(), group);
        }
        Ok(group)
    }

    /// Runs `parse` one group deeper, refusing nesting `fancy-regex` would reject.
    fn nested<T>(&mut self, start: usize, parse: impl FnOnce(&mut Self) -> RunResult<T>) -> RunResult<T> {
        if self.depth + 1 >= MAX_NESTING {
            return Err(self.source.error_at("pattern too deeply nested", start));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    /// Handles a `(` construct, port of the group branch of `_parse`.
    ///
    /// `first` is true where global flags are allowed: the start of the first top-level branch.
    fn group(&mut self, verbose: bool, nested: usize, first: bool) -> RunResult<GroupOutcome> {
        let start = self.source.tell() - 1;
        if !self.source.matches('?')? {
            let group = self.open_group(None)?;
            self.out.push('(');
            return self.group_body(start, verbose, nested, Some(group));
        }
        let Some(token) = self.source.get()? else {
            return Err(self.source.error("unexpected end of pattern", 0));
        };
        match token {
            Token::Char('P') => self.python_extension(start, verbose, nested),
            Token::Char(':') => {
                self.out.push_str("(?:");
                self.group_body(start, verbose, nested, None)
            }
            Token::Char('#') => loop {
                if self.source.next.is_none() {
                    let offset = self.source.tell() - start;
                    return Err(self.source.error("missing ), unterminated comment", offset));
                }
                if self.source.get()? == Some(Token::Char(')')) {
                    return Ok(GroupOutcome::Nothing);
                }
            },
            Token::Char(c @ ('=' | '!' | '<')) => self.lookaround(c, start, verbose, nested),
            Token::Char('(') => self.conditional(start, verbose, nested),
            Token::Char('>') => {
                self.out.push_str("(?>");
                self.group_body(start, verbose, nested, None)
            }
            Token::Char(c) if c == '-' || flag_bit(c).is_some() => self.flag_group(c, start, verbose, nested, first),
            other => Err(self
                .source
                .error(format!("unknown extension ?{other}"), other.len() + 1)),
        }
    }

    /// Parses a group's contents and its `)`, after the opening has been emitted.
    fn group_body(
        &mut self,
        start: usize,
        verbose: bool,
        nested: usize,
        group: Option<usize>,
    ) -> RunResult<GroupOutcome> {
        let width = self.nested(start, |this| this.parse_sub(verbose, nested + 1))?;
        self.close(start)?;
        if let Some(group) = group {
            self.group_widths[group] = Some(width);
        }
        Ok(GroupOutcome::Item(Item {
            kind: ItemKind::Other,
            width,
        }))
    }

    /// Consumes and emits the `)` closing the construct opened at `start`.
    fn close(&mut self, start: usize) -> RunResult<()> {
        if self.source.matches(')')? {
            self.out.push(')');
            Ok(())
        } else {
            let offset = self.source.tell() - start;
            Err(self.source.error("missing ), unterminated subpattern", offset))
        }
    }

    /// `(?P<name>...)` and `(?P=name)`.
    fn python_extension(&mut self, start: usize, verbose: bool, nested: usize) -> RunResult<GroupOutcome> {
        if self.source.matches('<')? {
            let name = self.source.get_until('>', "group name")?;
            self.source.check_group_name(&name, 1)?;
            let group = self.open_group(Some(&name))?;
            write!(self.out, "(?P<{name}>").expect("writing to a String is infallible");
            self.group_body(start, verbose, nested, Some(group))
        } else if self.source.matches('=')? {
            let name = self.source.get_until(')', "group name")?;
            self.source.check_group_name(&name, 1)?;
            let offset = name.chars().count() + 1;
            let Some(&group) = self.group_names.get(&name) else {
                let msg = format!("unknown group name {}", StringRepr(&name));
                return Err(self.source.error(msg, offset));
            };
            if !self.is_closed(group) {
                return Err(self.source.error("cannot refer to an open group", offset));
            }
            self.check_lookbehind_group(group)?;
            Ok(GroupOutcome::Item(self.push_backref(group)))
        } else {
            match self.source.get()? {
                None => Err(self.source.error("unexpected end of pattern", 0)),
                Some(c) => Err(self.source.error(format!("unknown extension ?P{c}"), c.len() + 2)),
            }
        }
    }

    /// Lookahead and lookbehind assertions, after `(?` and `c`.
    fn lookaround(&mut self, c: char, start: usize, verbose: bool, nested: usize) -> RunResult<GroupOutcome> {
        let (behind, kind) = if c == '<' {
            match self.source.get()? {
                None => return Err(self.source.error("unexpected end of pattern", 0)),
                Some(Token::Char(kind @ ('=' | '!'))) => (true, kind),
                Some(other) => {
                    return Err(self
                        .source
                        .error(format!("unknown extension ?<{other}"), other.len() + 2));
                }
            }
        } else {
            (false, c)
        };
        let outermost = behind && self.lookbehind_groups.is_none();
        if outermost {
            self.lookbehind_groups = Some(self.group_widths.len());
        }
        self.out.push_str(match (behind, kind) {
            (false, '=') => "(?=",
            (false, _) => "(?!",
            (true, '=') => "(?<=",
            (true, _) => "(?<!",
        });
        let width = self.nested(start, |this| this.parse_sub(verbose, nested + 1))?;
        if outermost {
            self.lookbehind_groups = None;
        }
        self.close(start)?;
        if behind {
            self.check_lookbehind_width(start, width);
        }
        Ok(GroupOutcome::Item(Item::EMPTY))
    }

    /// Records a lookbehind CPython's compiler rejects; it reports the first in pattern order.
    fn check_lookbehind_width(&mut self, start: usize, (lo, hi): Width) {
        let msg = if lo > MAXCODE {
            "looks too much behind"
        } else if lo != hi {
            "look-behind requires fixed-width pattern"
        } else {
            return;
        };
        if self.lookbehind_error.is_none_or(|(first, _)| start < first) {
            self.lookbehind_error = Some((start, msg));
        }
    }

    /// `(?(group)yes|no)`, after `(?(`.
    fn conditional(&mut self, start: usize, verbose: bool, nested: usize) -> RunResult<GroupOutcome> {
        let name = self.source.get_until(')', "group name")?;
        let offset = name.chars().count() + 1;
        let group = if name.bytes().all(|b| b.is_ascii_digit()) {
            let number = name.parse::<u64>().unwrap_or(u64::MAX);
            if number == 0 {
                return Err(self.source.error("bad group number", offset));
            }
            if number >= MAXGROUPS {
                let msg = format!("invalid group reference {}", name.trim_start_matches('0'));
                return Err(self.source.error(msg, offset));
            }
            // Repeats are kept rather than searched for: `finish` reports the first entry, so this matches
            // CPython's first-position dict without making many conditionals quadratic.
            self.groupref_pos.push((number, self.source.tell() - offset));
            usize::try_from(number).expect("below MAXGROUPS")
        } else {
            self.source.check_group_name(&name, 1)?;
            let Some(&group) = self.group_names.get(&name) else {
                let msg = format!("unknown group name {}", StringRepr(&name));
                return Err(self.source.error(msg, offset));
            };
            group
        };
        self.check_lookbehind_group(group)?;
        write!(self.out, "(?({group})").expect("writing to a String is infallible");
        let (yes, no) = self.nested(start, |this| {
            let yes = this.parse(verbose, nested + 1, false)?;
            if !this.source.matches('|')? {
                return Ok((yes, None));
            }
            this.out.push('|');
            let no = this.parse(verbose, nested + 1, false)?;
            if this.source.next == Some(Token::Char('|')) {
                return Err(this.source.error("conditional backref with more than two branches", 0));
            }
            Ok((yes, Some(no)))
        })?;
        self.close(start)?;
        let width = match no {
            Some(no) => (yes.0.min(no.0), yes.1.max(no.1)),
            None => (0, yes.1),
        };
        Ok(GroupOutcome::Item(Item {
            kind: ItemKind::Other,
            width,
        }))
    }

    /// Inline flags, global `(?flags)` or scoped `(?flags-flags:...)`, after `(?`.
    fn flag_group(
        &mut self,
        c: char,
        start: usize,
        verbose: bool,
        nested: usize,
        first: bool,
    ) -> RunResult<GroupOutcome> {
        match self.parse_flags(c)? {
            None => {
                if !first {
                    let offset = self.source.tell() - start;
                    return Err(self
                        .source
                        .error("global flags not at the start of the expression", offset));
                }
                Ok(GroupOutcome::GlobalFlags)
            }
            Some((add, del)) => {
                let (on, off) = (flag_letters(add), flag_letters(del));
                self.out.push_str("(?");
                self.out.push_str(&on);
                if !off.is_empty() {
                    self.out.push('-');
                    self.out.push_str(&off);
                }
                self.out.push(':');
                let verbose = (verbose || add & VERBOSE != 0) && del & VERBOSE == 0;
                self.group_body(start, verbose, nested, None)
            }
        }
    }

    /// Port of `_parse_flags`: `None` for global flags, which it applies and emits, else the scoped flags.
    fn parse_flags(&mut self, first: char) -> RunResult<Option<(u16, u16)>> {
        let mut add = 0;
        let mut current = Token::Char(first);
        if first != '-' {
            loop {
                let c = current.char();
                if c == 'L' {
                    return Err(self
                        .source
                        .error("bad inline flags: cannot use 'L' flag with a str pattern", 0));
                }
                let flag = flag_bit(c).expect("checked to be a flag");
                add |= flag;
                if flag & TYPE_FLAGS != 0 && add & TYPE_FLAGS != flag {
                    return Err(self
                        .source
                        .error("bad inline flags: flags 'a', 'u' and 'L' are incompatible", 0));
                }
                let Some(next) = self.source.get()? else {
                    return Err(self.source.error("missing -, : or )", 0));
                };
                current = next;
                if matches!(next, Token::Char(')' | '-' | ':')) {
                    break;
                }
                if !is_flag(next) {
                    let msg = if is_alpha(next) {
                        "unknown flag"
                    } else {
                        "missing -, : or )"
                    };
                    return Err(self.source.error(msg, next.len()));
                }
            }
        }
        if current == Token::Char(')') {
            self.flags |= add;
            let letters = flag_letters(add);
            if !letters.is_empty() {
                write!(self.out, "(?{letters})").expect("writing to a String is infallible");
            }
            return Ok(None);
        }
        let mut del = 0;
        if current == Token::Char('-') {
            let Some(mut next) = self.source.get()? else {
                return Err(self.source.error("missing flag", 0));
            };
            if !is_flag(next) {
                let msg = if is_alpha(next) { "unknown flag" } else { "missing flag" };
                return Err(self.source.error(msg, next.len()));
            }
            loop {
                let flag = flag_bit(next.char()).expect("checked to be a flag");
                if flag & TYPE_FLAGS != 0 {
                    return Err(self
                        .source
                        .error("bad inline flags: cannot turn off flags 'a', 'u' and 'L'", 0));
                }
                del |= flag;
                let Some(after) = self.source.get()? else {
                    return Err(self.source.error("missing :", 0));
                };
                if after == Token::Char(':') {
                    break;
                }
                if !is_flag(after) {
                    let msg = if is_alpha(after) { "unknown flag" } else { "missing :" };
                    return Err(self.source.error(msg, after.len()));
                }
                next = after;
            }
        }
        if add & del != 0 {
            return Err(self.source.error("bad inline flags: flag turned on and off", 1));
        }
        Ok(Some((add, del)))
    }
}

/// The `re` flag bit for an inline flag letter.
fn flag_bit(c: char) -> Option<u16> {
    match c {
        'i' => Some(IGNORECASE),
        'L' => Some(LOCALE),
        'm' => Some(MULTILINE),
        's' => Some(DOTALL),
        'x' => Some(VERBOSE),
        'a' => Some(ASCII),
        'u' => Some(UNICODE),
        _ => None,
    }
}

/// True for a plain character that is an inline flag letter.
fn is_flag(token: Token) -> bool {
    matches!(token, Token::Char(c) if flag_bit(c).is_some())
}

/// Python's `str.isalpha()` on the token's text.
fn is_alpha(token: Token) -> bool {
    matches!(token, Token::Char(c) if c.is_alphabetic())
}

/// The `fancy-regex` letters for `flags`: only `i`, `m` and `s` change how the output matches.
fn flag_letters(flags: u16) -> String {
    [(IGNORECASE, 'i'), (MULTILINE, 'm'), (DOTALL, 's')]
        .into_iter()
        .filter(|&(bit, _)| flags & bit != 0)
        .map(|(_, letter)| letter)
        .collect()
}

/// A repeat count, which must be below [`MAXREPEAT`].
fn repeat_count(digits: &str) -> RunResult<u64> {
    digits
        .parse::<u64>()
        .ok()
        .filter(|&n| n < MAXREPEAT)
        .ok_or_else(|| ExcType::overflow_error("the repetition number is too large"))
}

/// Appends `c` as a literal, in a form `fancy-regex` cannot read as syntax.
fn push_literal(out: &mut String, cp: u32) {
    match char::from_u32(cp) {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii() => out.push(c),
        Some(c) => push_hex(out, c),
        // A surrogate: no Monty string contains one, so the literal never matches.
        None => out.push_str("[^\\s\\S]"),
    }
}

/// Appends `\x{..}` for `c`.
fn push_hex(out: &mut String, c: char) {
    write!(out, "\\x{{{:x}}}", u32::from(c)).expect("writing to a String is infallible");
}

/// Appends a character class; surrogates are dropped, since no Monty string contains one.
fn push_class(out: &mut String, negate: bool, members: &[ClassItem]) {
    let mut body = String::new();
    for &member in members {
        match member {
            ClassItem::Literal(cp) => {
                if let Some(c) = char::from_u32(cp) {
                    push_class_char(&mut body, c);
                }
            }
            ClassItem::Range(lo, hi) => {
                let lo = if (0xD800..=0xDFFF).contains(&lo) { 0xE000 } else { lo };
                let hi = if (0xD800..=0xDFFF).contains(&hi) { 0xD7FF } else { hi };
                if let (Some(lo), Some(hi)) = (char::from_u32(lo), char::from_u32(hi))
                    && lo <= hi
                {
                    push_class_char(&mut body, lo);
                    body.push('-');
                    push_class_char(&mut body, hi);
                }
            }
            ClassItem::Category(c) => {
                body.push('\\');
                body.push(c);
            }
        }
    }
    match (body.is_empty(), negate) {
        // Only surrogates: the class matches nothing, or anything when negated.
        (true, false) => out.push_str("[^\\s\\S]"),
        (true, true) => out.push_str("[\\s\\S]"),
        (false, _) => {
            out.push('[');
            if negate {
                out.push('^');
            }
            out.push_str(&body);
            out.push(']');
        }
    }
}

/// Appends a class member character, escaping the punctuation `fancy-regex` treats as class syntax.
fn push_class_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii() {
        out.push(c);
    } else {
        push_hex(out, c);
    }
}

/// The quantifier for `{min,max}`, using the short forms where they apply.
fn quantifier(min: u64, max: u64) -> String {
    match (min, max) {
        (0, MAXREPEAT) => "*".to_owned(),
        (1, MAXREPEAT) => "+".to_owned(),
        (0, 1) => "?".to_owned(),
        (min, MAXREPEAT) => format!("{{{min},}}"),
        (min, max) if min == max => format!("{{{min}}}"),
        (min, max) => format!("{{{min},{max}}}"),
    }
}

/// Adds two widths, saturating at [`MAXWIDTH`].
fn add_widths(a: Width, b: Width) -> Width {
    ((a.0 + b.0).min(MAXWIDTH), (a.1 + b.1).min(MAXWIDTH))
}
