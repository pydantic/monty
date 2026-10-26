//! `_csv.reader`: parses records from an iterator of lines.
//!
//! The parser is a port of CPython's `parse_process_char` state machine, so quoting,
//! escaping, `skipinitialspace`, `strict` and the `QUOTE_*` conversions behave identically.
//! A record may span several source lines (a quoted field containing newlines), so one
//! `__next__` pulls lines until the machine returns to `StartRecord`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, Heap, HeapData, HeapId, HeapReadMut},
    types::{
        CsvObject, List,
        csv::dialect::{Dialect, Quoting},
        str::allocate_string,
        r#type::parse_f64_from_str,
    },
    value::{VALUE_SIZE, Value},
};

/// CPython's default `csv.field_size_limit()`; Monty does not let it change.
pub(crate) const FIELD_SIZE_LIMIT: usize = 131_072;

/// A `_csv.reader`: the source iterator, the dialect, and the lines consumed so far.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Reader {
    /// The iterator lines come from (`iter(csvfile)` at construction). Owned.
    source: Value,
    /// Shared with each record read, so a row costs no copy of the dialect's strings.
    pub(crate) dialect: Arc<Dialect>,
    /// Source lines consumed, including those a multi-line record spans.
    pub(crate) line_num: u64,
    /// Fields in the last record: the capacity the next record's vector starts at, since
    /// records of one source are nearly always the same width.
    #[serde(skip)]
    field_hint: usize,
}

impl Reader {
    pub(crate) fn new(source: Value, dialect: Dialect) -> Self {
        Self {
            source,
            dialect: Arc::new(dialect),
            line_num: 0,
            field_hint: 0,
        }
    }

    /// Invokes `on_child` for each heap id this reader owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the source reference (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.source.py_dec_ref_ids(stack);
    }
}

/// One record as a `list` of fields, or `None` once the source is exhausted.
pub(super) fn next<'h>(obj: &HeapReadMut<'h, CsvObject>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let fields = next_fields(obj, vm)?;
    Ok(fields.map(|fields| Value::Ref(vm.heap.allocate(HeapData::List(List::new(fields))))))
}

/// The fields of the next record, or `None` once the source is exhausted. `DictReader`
/// takes the fields this way, so its rows never pass through a list.
pub(super) fn next_fields<'h>(obj: &HeapReadMut<'h, CsvObject>, vm: &mut VM<'h>) -> RunResult<Option<Vec<Value>>> {
    let (source, dialect, field_hint) = {
        let CsvObject::Reader(reader) = obj.get(vm.heap) else {
            unreachable!("dispatched on CsvObject::Reader")
        };
        (
            reader.source.clone_with_heap(vm.heap),
            Arc::clone(&reader.dialect),
            reader.field_hint,
        )
    };
    defer_drop!(source, vm);
    vm.heap.tracker.check_allocation(field_hint * VALUE_SIZE)?;
    let mut fields_guard = DropGuard::new(Vec::with_capacity(field_hint), vm);
    let (fields, vm) = fields_guard.as_parts_mut();
    let mut parser = Parser::new(&dialect);

    // Native loops over source lines and their characters: poll the tracker so time
    // limits still bite on one enormous line.
    let mut polls = 0usize;
    loop {
        vm.heap.tracker.check_time_every(polls)?;
        polls += 1;
        let line = {
            let mut guard = vm.recursion_guard()?;
            let vm = &mut *guard;
            source.read(vm).py_next(vm)?
        };
        let Some(line) = line else {
            // End of input mid-record: strict mode rejects it, otherwise the partial field closes the record.
            if parser.field_len != 0 || parser.state == State::InQuotedField {
                if dialect.strict {
                    return Err(ExcType::csv_error("unexpected end of data"));
                }
                parser.save_field(fields, vm.heap)?;
                break;
            }
            return Ok(None);
        };
        defer_drop!(line, vm);
        if !line.is_str(vm.heap) {
            return Err(ExcType::csv_error(format!(
                "iterator should return strings, not {} (the file should be opened in text mode)",
                line.py_type_name(vm)
            )));
        }
        if let CsvObject::Reader(reader) = obj.get_mut(vm.heap) {
            reader.line_num += 1;
        }
        // The parser only allocates, so the line is read in place rather than copied.
        let heap: &Heap = vm.heap;
        let mut rest = line.to_str_heap(heap, vm.interns)?;
        while !rest.is_empty() {
            heap.tracker.check_time_every(polls)?;
            polls += 1;
            // Inside a field, everything up to the next character the dialect treats
            // specially goes in as one run; any other state moves one character.
            let run = parser.run_len(rest);
            if run > 0 {
                parser.add_run(&rest[..run])?;
                rest = &rest[run..];
            } else {
                let c = rest.chars().next().expect("rest is not empty");
                rest = &rest[c.len_utf8()..];
                parser.process_char(Some(c), fields, heap)?;
            }
        }
        parser.process_char(None, fields, heap)?;
        if parser.state == State::StartRecord {
            break;
        }
    }
    let (fields, vm) = fields_guard.into_parts();
    if let CsvObject::Reader(reader) = obj.get_mut(vm.heap) {
        reader.field_hint = fields.len();
    }
    Ok(Some(fields))
}

/// The parser states of CPython's `_csv.c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    EscapedChar,
    InField,
    InQuotedField,
    EscapeInQuotedField,
    QuoteInQuotedField,
    EatCrNl,
    AfterEscapedCrNl,
}

/// Per-record parse state. Completed fields go to the caller's guarded `Vec`, so the
/// parser itself holds no heap references.
struct Parser<'d> {
    dialect: &'d Dialect,
    state: State,
    /// The field being built; kept between fields so its capacity is reused.
    field: String,
    /// Characters in `field`, checked against [`FIELD_SIZE_LIMIT`].
    field_len: usize,
    /// Whether the current field started without a quote: drives the `QUOTE_NONNUMERIC`,
    /// `QUOTE_STRINGS` and `QUOTE_NOTNULL` conversions.
    unquoted_field: bool,
}

impl<'d> Parser<'d> {
    fn new(dialect: &'d Dialect) -> Self {
        Self {
            dialect,
            state: State::StartRecord,
            field: String::new(),
            field_len: 0,
            unquoted_field: false,
        }
    }

    /// Feeds one character, or `None` for the end of a source line.
    fn process_char(&mut self, c: Option<char>, fields: &mut Vec<Value>, heap: &Heap) -> RunResult<()> {
        let d = self.dialect;
        // The state a line break leads to: the record is complete at end of line,
        // otherwise the rest of the terminator is swallowed.
        let after_break = |c: Option<char>| {
            if c.is_none() {
                State::StartRecord
            } else {
                State::EatCrNl
            }
        };

        match self.state {
            State::StartRecord => match c {
                // An empty line is an empty record.
                None => {}
                Some('\n' | '\r') => self.state = State::EatCrNl,
                Some(_) => {
                    self.state = State::StartField;
                    return self.process_char(c, fields, heap);
                }
            },
            State::StartField => {
                self.unquoted_field = true;
                match c {
                    None | Some('\n' | '\r') => {
                        self.save_field(fields, heap)?;
                        self.state = after_break(c);
                    }
                    Some(ch) if self.is_quote(ch) => {
                        self.unquoted_field = false;
                        self.state = State::InQuotedField;
                    }
                    Some(ch) if Some(ch) == d.escapechar => self.state = State::EscapedChar,
                    // Leading spaces are skipped.
                    Some(' ') if d.skipinitialspace => {}
                    Some(ch) if ch == d.delimiter => self.save_field(fields, heap)?,
                    Some(ch) => {
                        self.add_char(ch)?;
                        self.state = State::InField;
                    }
                }
            }
            State::EscapedChar => {
                if let Some(ch @ ('\n' | '\r')) = c {
                    self.add_char(ch)?;
                    self.state = State::AfterEscapedCrNl;
                } else {
                    self.add_char(c.unwrap_or('\n'))?;
                    self.state = State::InField;
                }
            }
            State::AfterEscapedCrNl => {
                if c.is_some() {
                    self.state = State::InField;
                    return self.process_char(c, fields, heap);
                }
            }
            State::InField => match c {
                None | Some('\n' | '\r') => {
                    self.save_field(fields, heap)?;
                    self.state = after_break(c);
                }
                Some(ch) if Some(ch) == d.escapechar => self.state = State::EscapedChar,
                Some(ch) if ch == d.delimiter => {
                    self.save_field(fields, heap)?;
                    self.state = State::StartField;
                }
                Some(ch) => self.add_char(ch)?,
            },
            State::InQuotedField => match c {
                // The record continues on the next source line.
                None => {}
                Some(ch) if Some(ch) == d.escapechar => self.state = State::EscapeInQuotedField,
                Some(ch) if self.is_quote(ch) => {
                    self.state = if d.doublequote {
                        State::QuoteInQuotedField
                    } else {
                        State::InField
                    };
                }
                Some(ch) => self.add_char(ch)?,
            },
            State::EscapeInQuotedField => {
                self.add_char(c.unwrap_or('\n'))?;
                self.state = State::InQuotedField;
            }
            State::QuoteInQuotedField => match c {
                Some(ch) if self.is_quote(ch) => {
                    self.add_char(ch)?;
                    self.state = State::InQuotedField;
                }
                Some(ch) if ch == d.delimiter => {
                    self.save_field(fields, heap)?;
                    self.state = State::StartField;
                }
                None | Some('\n' | '\r') => {
                    self.save_field(fields, heap)?;
                    self.state = after_break(c);
                }
                Some(_) if d.strict => {
                    return Err(ExcType::csv_error(format!(
                        "'{}' expected after '{}'",
                        d.delimiter,
                        d.quotechar.expect("quoting is enabled")
                    )));
                }
                Some(ch) => {
                    self.add_char(ch)?;
                    self.state = State::InField;
                }
            },
            State::EatCrNl => match c {
                // The rest of a `\r\n` terminator.
                Some('\n' | '\r') => {}
                None => self.state = State::StartRecord,
                Some(_) => {
                    return Err(ExcType::csv_error(
                        "new-line character seen in unquoted field - do you need to open the file with newline=''?",
                    ));
                }
            },
        }
        Ok(())
    }

    /// Bytes at the start of `rest` that the current field absorbs as ordinary text: up to
    /// the next delimiter, escape or line break in a plain field, up to the next quote or
    /// escape in a quoted one. Zero in every other state, where each character is a step.
    fn run_len(&self, rest: &str) -> usize {
        let d = self.dialect;
        match self.state {
            State::InField => {
                rest.find(|c: char| matches!(c, '\n' | '\r') || Some(c) == d.escapechar || c == d.delimiter)
            }
            State::InQuotedField => rest.find(|c: char| Some(c) == d.escapechar || self.is_quote(c)),
            _ => Some(0),
        }
        .unwrap_or(rest.len())
    }

    /// Whether `c` opens or closes a quoted field under this dialect.
    fn is_quote(&self, c: char) -> bool {
        Some(c) == self.dialect.quotechar && self.dialect.quoting != Quoting::None
    }

    /// Appends a run of ordinary characters to the current field, within the field size limit.
    fn add_run(&mut self, run: &str) -> RunResult<()> {
        let count = run.chars().count();
        if self.field_len + count > FIELD_SIZE_LIMIT {
            Err(field_limit_error())
        } else {
            self.field.push_str(run);
            self.field_len += count;
            Ok(())
        }
    }

    /// Appends a character to the current field, within the field size limit.
    fn add_char(&mut self, c: char) -> RunResult<()> {
        if self.field_len >= FIELD_SIZE_LIMIT {
            Err(field_limit_error())
        } else {
            self.field.push(c);
            self.field_len += 1;
            Ok(())
        }
    }

    /// Closes the current field: an unquoted empty field is `None` under `QUOTE_NOTNULL` /
    /// `QUOTE_STRINGS`, an unquoted non-empty one a `float` under `QUOTE_NONNUMERIC` / `QUOTE_STRINGS`.
    fn save_field(&mut self, fields: &mut Vec<Value>, heap: &Heap) -> RunResult<()> {
        let quoting = self.dialect.quoting;
        let unquoted = self.unquoted_field;
        let value = if unquoted && self.field_len == 0 && matches!(quoting, Quoting::Notnull | Quoting::Strings) {
            Value::None
        } else if unquoted && self.field_len != 0 && matches!(quoting, Quoting::Nonnumeric | Quoting::Strings) {
            Value::Float(parse_f64_from_str(&self.field)?)
        } else {
            // From the slice: one exact-size allocation, and the buffer keeps its capacity.
            allocate_string(self.field.as_str(), heap)
        };
        self.field.clear();
        self.field_len = 0;
        self.unquoted_field = false;
        heap.tracker.check_growth(fields.len(), fields.capacity(), VALUE_SIZE)?;
        fields.push(value);
        Ok(())
    }
}

/// The `_csv.Error` for a field past [`FIELD_SIZE_LIMIT`].
fn field_limit_error() -> RunError {
    ExcType::csv_error(format!("field larger than field limit ({FIELD_SIZE_LIMIT})"))
}
