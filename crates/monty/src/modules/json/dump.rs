//! JSON serialization support for `json.dumps()` and `json.dump()`.
//!
//! This module owns encoder keyword parsing, CPython-compatible string/float
//! formatting, and recursive serialization of Monty values. `dump` encodes
//! the whole document first and hands it to the file object in one write
//! (see [`FileTarget`]).

use std::fmt::Write;

use super::fp::FileTarget;
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, ContainsVM, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapRead},
    py_value::PyValue,
    sorting::{apply_permutation, sort_indices},
    types::{Dict, PyTrait, Type, str::allocate_string},
    value::Value,
};

/// Serializer configuration derived from `json.dumps()` keyword arguments.
///
/// The struct stores only the subset of encoder configuration that this module
/// actually uses while serializing. Unsupported or not-yet-implemented kwargs
/// still raise during parsing so call sites do not silently lose behavior.
struct JsonDumpsConfig {
    indent: Option<String>,
    item_separator: String,
    key_separator: String,
    flags: u8,
}

impl Default for JsonDumpsConfig {
    /// Returns the CPython default `json.dumps()` configuration.
    ///
    /// Compact output uses `", "` between items and `": "` between keys and
    /// values, ASCII escaping is enabled, NaN and infinity are emitted as
    /// `NaN`/`Infinity`, and invalid dict keys raise immediately.
    fn default() -> Self {
        Self {
            indent: None,
            item_separator: ", ".to_owned(),
            key_separator: ": ".to_owned(),
            flags: Self::ENSURE_ASCII | Self::ALLOW_NAN,
        }
    }
}

impl JsonDumpsConfig {
    /// Bit flag storing the `sort_keys` option.
    const SORT_KEYS: u8 = 1 << 0;
    /// Bit flag storing the `ensure_ascii` option.
    const ENSURE_ASCII: u8 = 1 << 1;
    /// Bit flag storing the `allow_nan` option.
    const ALLOW_NAN: u8 = 1 << 2;
    /// Bit flag storing the `skipkeys` option.
    const SKIPKEYS: u8 = 1 << 3;

    /// Returns whether `sort_keys=True` is enabled.
    fn sort_keys(&self) -> bool {
        self.flags & Self::SORT_KEYS != 0
    }

    /// Returns whether non-ASCII characters must be escaped.
    fn ensure_ascii(&self) -> bool {
        self.flags & Self::ENSURE_ASCII != 0
    }

    /// Returns whether NaN and infinity may be emitted as JSON tokens.
    fn allow_nan(&self) -> bool {
        self.flags & Self::ALLOW_NAN != 0
    }

    /// Returns whether unsupported dict keys should be skipped.
    fn skipkeys(&self) -> bool {
        self.flags & Self::SKIPKEYS != 0
    }

    /// Builds the encoder config from the bound keyword arguments.
    ///
    /// The `FromArgs` structs already bound the arguments and rejected unknown
    /// keywords. This translates the raw values into encoder options, raising in
    /// CPython's order: `separators` is unpacked by `JSONEncoder.__init__`, then
    /// `' ' * indent` and the separator types are checked as the encoder is built;
    /// `checks` says how much of that a call reaches.
    fn from_kwargs(kwargs: &EncoderKwargs, checks: EncoderChecks, vm: &mut VM<'_>) -> RunResult<Self> {
        let mut config = Self::default();

        let separators_were_set = check_separators_shape(&kwargs.separators, vm)?;
        if checks != EncoderChecks::Unpack {
            config.indent = parse_indent_value(&kwargs.indent, vm)?;
        }
        // `separators=None` means the indent-aware defaults below.
        if checks == EncoderChecks::All
            && let Some((item, key)) = parse_separators_value(&kwargs.separators, vm)?
        {
            config.item_separator = item;
            config.key_separator = key;
        }

        config.flags = apply_bool_flag(config.flags, Self::SORT_KEYS, &kwargs.sort_keys, vm)?;
        config.flags = apply_bool_flag(config.flags, Self::ENSURE_ASCII, &kwargs.ensure_ascii, vm)?;
        config.flags = apply_bool_flag(config.flags, Self::ALLOW_NAN, &kwargs.allow_nan, vm)?;
        config.flags = apply_bool_flag(config.flags, Self::SKIPKEYS, &kwargs.skipkeys, vm)?;

        if config.indent.is_some() && !separators_were_set {
            ",".clone_into(&mut config.item_separator);
            ": ".clone_into(&mut config.key_separator);
        }

        Ok(config)
    }
}

/// How far a call gets through CPython's encoder setup before encoding, which
/// decides the keyword errors it can raise.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EncoderChecks {
    /// `json.dumps` of a `str` returns before building an encoder.
    Unpack,
    /// `json.dump` of a `str`: the pure-Python encoder builds `' ' * indent`
    /// but never uses the separators.
    Indent,
    /// Every other call: the separators must be `str`.
    All,
}

impl EncoderChecks {
    /// The checks `json.dumps` (`dump = false`) or `json.dump` makes for `obj`.
    fn for_call(obj: &Value, dump: bool, vm: &VM<'_>) -> Self {
        match (obj.is_str(vm.heap), dump) {
            (false, _) => Self::All,
            (true, false) => Self::Unpack,
            (true, true) => Self::Indent,
        }
    }
}

/// The encoder keyword arguments as bound by [`JsonDumpsArgs`] / [`JsonDumpArgs`],
/// before [`JsonDumpsConfig::from_kwargs`] coerces them. Callers hold it under
/// `defer_drop!` so a coercion error part-way through releases every field.
struct EncoderKwargs {
    indent: Value,
    sort_keys: Value,
    ensure_ascii: Value,
    allow_nan: Value,
    separators: Value,
    skipkeys: Value,
}

impl<C: ContainsHeap> DropWithContext<C> for EncoderKwargs {
    fn drop_with(self, ctx: &mut C) {
        self.indent.drop_with(ctx);
        self.sort_keys.drop_with(ctx);
        self.ensure_ascii.drop_with(ctx);
        self.allow_nan.drop_with(ctx);
        self.separators.drop_with(ctx);
        self.skipkeys.drop_with(ctx);
    }
}

/// Implements `json.dumps(obj, **kwargs)`.
///
/// Only the first argument may be positional. Supported keyword arguments mirror
/// the high-value subset of CPython's encoder configuration: `indent`,
/// `sort_keys`, `ensure_ascii`, `allow_nan`, `separators`, and `skipkeys`.
///
/// CPython kwargs `cls`, `default`, and `check_circular` are intentionally
/// unsupported and will raise `TypeError` if passed.
pub(super) fn call_dumps(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let JsonDumpsArgs {
        obj,
        indent,
        sort_keys,
        ensure_ascii,
        allow_nan,
        separators,
        skipkeys,
    } = JsonDumpsArgs::from_args(args, vm)?;
    // `obj` outlives kwarg processing: an early error below must not leak it.
    defer_drop!(obj, vm);
    let kwargs = EncoderKwargs {
        indent,
        sort_keys,
        ensure_ascii,
        allow_nan,
        separators,
        skipkeys,
    };
    defer_drop!(kwargs, vm);
    let config = JsonDumpsConfig::from_kwargs(kwargs, EncoderChecks::for_call(obj, false, vm), vm)?;
    let output = encode(obj, &config, vm)?;
    Ok(allocate_string(output, vm.heap))
}

/// Implements `json.dump(obj, fp, **kwargs)` with the same keyword subset as
/// [`call_dumps`]: the document is encoded in full, then written to `fp` as
/// one `write()` call, and the function evaluates to `None`.
///
/// An `open()` handle takes the one-shot host write (the call suspends like
/// `fp.write` would); any other object has its `write` attribute called
/// synchronously. CPython streams chunks instead, so it reaches `fp.write`
/// before a bad element deep in a container raises, but after a scalar that
/// cannot be encoded; the file object is checked first or last to match.
pub(super) fn call_dump(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let JsonDumpArgs {
        obj,
        fp,
        indent,
        sort_keys,
        ensure_ascii,
        allow_nan,
        separators,
        skipkeys,
    } = JsonDumpArgs::from_args(args, vm)?;
    defer_drop!(obj, vm);
    defer_drop!(fp, vm);
    let kwargs = EncoderKwargs {
        indent,
        sort_keys,
        ensure_ascii,
        allow_nan,
        separators,
        skipkeys,
    };
    defer_drop!(kwargs, vm);
    let config = JsonDumpsConfig::from_kwargs(kwargs, EncoderChecks::for_call(obj, true, vm), vm)?;

    let early_target = if is_container(obj, vm) {
        Some(FileTarget::writer(fp, vm)?)
    } else {
        None
    };
    // Released if encoding fails, otherwise taken back to write to.
    let mut early_target = DropGuard::new(early_target, vm);
    let output = encode(obj, &config, early_target.ctx())?;
    let (early_target, vm) = early_target.into_parts();
    let target = match early_target {
        Some(target) => target,
        None => FileTarget::writer(fp, vm)?,
    };
    target.write(output, vm)
}

/// Serializes `obj` to JSON text under `config`.
fn encode(obj: &Value, config: &JsonDumpsConfig, vm: &mut VM<'_>) -> RunResult<String> {
    let mut output = String::new();
    let mut active_containers = Vec::new();
    let mut encoder = Encoder {
        out: &mut output,
        config,
        active_containers: &mut active_containers,
        vm,
    };
    encoder.serialize_value(obj, 0)?;
    Ok(output)
}

/// Whether CPython's encoder would yield a chunk of `value` (its opening
/// bracket) before any of its elements could fail to encode.
fn is_container(value: &Value, vm: &VM<'_>) -> bool {
    match value {
        Value::Ref(heap_id) => matches!(
            vm.heap.get(*heap_id),
            HeapData::List(_) | HeapData::Tuple(_) | HeapData::Dict(_)
        ),
        _ => false,
    }
}

/// Argument shape for `json.dumps(obj, *, indent=None, sort_keys=False,
/// ensure_ascii=True, allow_nan=True, separators=None, skipkeys=False)`.
///
/// Arity and missing-arg errors use the `dumps()` descriptor with `style =
/// def` wording (CPython's `json.dumps` is a pure-Python `def`), but the
/// unknown-kwarg error uses `JSONEncoder.__init__()` — CPython's `json.dumps`
/// forwards unknown kwargs straight to the encoder constructor, which is what
/// surfaces in the error. `kwarg_error_name` overrides the function name used
/// in the unexpected-keyword message without affecting other error paths.
/// Every field is a raw `Value` so the encoder can apply its own truth-test
/// (`py_bool`) or shape coercion (`parse_indent_value` /
/// `parse_separators_value`) on the way through.
#[derive(FromArgs)]
#[from_args(name = "dumps", style = def, kwarg_error_name = "JSONEncoder.__init__")]
struct JsonDumpsArgs {
    obj: Value,
    #[from_args(kw_only, default = Value::None)]
    indent: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    sort_keys: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    ensure_ascii: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    allow_nan: Value,
    #[from_args(kw_only, default = Value::None)]
    separators: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    skipkeys: Value,
}

/// Argument shape for `json.dump(obj, fp, *, …)`: [`JsonDumpsArgs`] with the
/// file object as second positional, under the `dump()` descriptor.
#[derive(FromArgs)]
#[from_args(name = "dump", style = def, kwarg_error_name = "JSONEncoder.__init__")]
struct JsonDumpArgs {
    obj: Value,
    fp: Value,
    #[from_args(kw_only, default = Value::None)]
    indent: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    sort_keys: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    ensure_ascii: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    allow_nan: Value,
    #[from_args(kw_only, default = Value::None)]
    separators: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    skipkeys: Value,
}

/// Sets `bit` in `flags` when `value` is truthy, clearing it otherwise. Used by the json.dumps kwarg pipeline so each
/// boolean-style flag is handled with a single line.
fn apply_bool_flag(flags: u8, bit: u8, value: &Value, vm: &mut VM<'_>) -> RunResult<u8> {
    if value.py_bool(vm)? {
        Ok(flags | bit)
    } else {
        Ok(flags & !bit)
    }
}

/// Parses the `indent=` value for `json.dumps()`.
///
/// `None` keeps compact mode, a string is repeated once per depth level, and
/// any other value becomes CPython's `' ' * indent`: integers give that many
/// spaces (none when zero or negative) and other types raise its `TypeError`.
fn parse_indent_value(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<String>> {
    match value.py_value(vm) {
        PyValue::None => Ok(None),
        PyValue::Bool(value) => spaces_from_indent_count(i64::from(value)),
        PyValue::Int(integer) => {
            spaces_from_indent_count(integer.to_i64(vm).ok_or_else(ExcType::overflow_index_sized_int)?)
        }
        PyValue::Str(value) => Ok(Some(value.as_str(vm).to_owned())),
        _ => Err(ExcType::type_error_sequence_repeat(value.py_type_name(vm))),
    }
}

/// Builds the `' ' * count` indent string.
fn spaces_from_indent_count(count: i64) -> RunResult<Option<String>> {
    // CPython repeats a negative count to the empty string.
    let count = if count < 0 {
        0
    } else {
        usize::try_from(count).map_err(|_| ExcType::overflow_index_sized_int())?
    };
    Ok(Some(" ".repeat(count)))
}

/// Raises the unpacking errors `JSONEncoder.__init__` gives `separators`,
/// returning whether a pair was passed.
fn check_separators_shape(value: &Value, vm: &VM<'_>) -> RunResult<bool> {
    let len = match value {
        Value::None => return Ok(false),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Tuple(tuple) => tuple.as_slice().len(),
            HeapData::List(list) => list.as_slice().len(),
            _ => return Err(separators_not_iterable(value, vm)),
        },
        _ => return Err(separators_not_iterable(value, vm)),
    };
    check_separators_length(len)?;
    Ok(true)
}

/// CPython's error unpacking a non-iterable `separators` value.
fn separators_not_iterable(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!("cannot unpack non-iterable {} object", value.py_type_name(vm)))
}

/// Converts the `separators=` pair for the encoder, after
/// [`check_separators_shape`] has accepted it; `None` keeps the defaults.
fn parse_separators_value(value: &Value, vm: &VM<'_>) -> RunResult<Option<(String, String)>> {
    let Value::Ref(heap_id) = value else {
        return Ok(None);
    };
    let items = match vm.heap.get(*heap_id) {
        HeapData::Tuple(tuple) => tuple.as_slice(),
        HeapData::List(list) => list.as_slice(),
        _ => return Ok(None),
    };
    Ok(Some((
        json_separator_to_string(&items[0], "item_separator", vm)?,
        json_separator_to_string(&items[1], "key_separator", vm)?,
    )))
}

/// Validates that the separators sequence has exactly two elements.
///
/// Raises `ValueError` with the same unpacking-style message as CPython when
/// the length does not match the expected two elements.
fn check_separators_length(len: usize) -> RunResult<()> {
    if len == 2 {
        Ok(())
    } else {
        Err(ExcType::unpack_size_error(2, len))
    }
}

/// Converts a Monty value to a string for use as a JSON separator.
///
/// CPython's C encoder validates separators as strings and refers to them by
/// their positional argument index in `make_encoder()`. The `role` parameter
/// selects the matching CPython argument number (6 for `item_separator`,
/// 5 for `key_separator`) so the error message matches CPython exactly.
fn json_separator_to_string(value: &Value, role: &str, vm: &VM<'_>) -> RunResult<String> {
    let arg_num = if role == "item_separator" { 6 } else { 5 };
    match value {
        Value::InternString(string_id) => Ok(vm.interns.get_str(*string_id).to_owned()),
        Value::Ref(heap_id) => match vm.heap.get(*heap_id) {
            HeapData::Str(string) => Ok(string.as_str().to_owned()),
            _ => Err(ExcType::type_error(format!(
                "make_encoder() argument {arg_num} must be str, not {}",
                value.py_type_name(vm)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "make_encoder() argument {arg_num} must be str, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// Mutable serialization context shared across the recursive `serialize_*`
/// methods.
///
/// Groups the four pieces of per-call state that every recursive level needs
/// — the JSON output buffer, the encoder configuration, the cycle-detection
/// stack, and the VM (heap + interns) — so the recursive methods can be
/// invoked as `encoder.serialize_value(value, depth)` rather than threading
/// five separate parameters through every signature.
///
/// `depth` is intentionally **not** a field of this struct: every recursive
/// call needs to bump it by one for the descent and restore it on return,
/// which is exactly what a stack-passed parameter does for free.
struct Encoder<'a, 'h> {
    out: &'a mut String,
    config: &'a JsonDumpsConfig,
    active_containers: &'a mut Vec<HeapId>,
    vm: &'a mut VM<'h>,
}

/// Lets the encoder participate in the `DropGuard` / [`defer_drop_mut!`]
/// pattern: passing the encoder as the "heap" argument re-borrows the whole
/// encoder (including its `vm`) into the guard, which is exactly what we need
/// so the rebound iter and the rebound encoder share a lifetime.
impl ContainsHeap for Encoder<'_, '_> {
    fn heap(&self) -> &Heap {
        self.vm.heap()
    }

    fn heap_mut(&mut self) -> &mut Heap {
        self.vm.heap_mut()
    }
}

/// Lets a [`RecursionToken`](crate::bytecode::RecursionToken) (and the container
/// iterators that hold one) be released through the encoder via `defer_drop!`,
/// reaching the VM-side recursion counter while the encoder itself stays borrowable.
impl<'h> ContainsVM<'h> for Encoder<'_, 'h> {
    fn vm(&mut self) -> &mut VM<'h> {
        self.vm
    }
}

impl<'h> Encoder<'_, 'h> {
    /// Serializes a Monty value into JSON text.
    ///
    /// Handles immediate primitives directly and delegates to type-specific
    /// helpers for strings, long integers, lists, tuples, and dicts.
    fn serialize_value(&mut self, value: &Value, depth: usize) -> RunResult<()> {
        let view = value.py_value(self.vm);
        // Strings size their own check, and containers check per item.
        if matches!(
            view,
            PyValue::None | PyValue::Bool(_) | PyValue::Int(_) | PyValue::Float(_)
        ) {
            self.check_output_growth(0)?;
        }
        match view {
            PyValue::None => {
                self.out.push_str("null");
                Ok(())
            }
            PyValue::Bool(true) => {
                self.out.push_str("true");
                Ok(())
            }
            PyValue::Bool(false) => {
                self.out.push_str("false");
                Ok(())
            }
            PyValue::Int(integer) => integer.write_decimal(self.out, self.vm),
            PyValue::Float(value) => serialize_float(value, self.out, self.config),
            PyValue::Str(string) => {
                self.check_output_growth(string.as_str(self.vm).len())?;
                write_json_string(string.as_str(self.vm), self.out, self.config.ensure_ascii());
                Ok(())
            }
            PyValue::List(list) => self.with_entered_container(list.id(), |enc| {
                let iter = list.iter(enc.vm)?;
                defer_drop_mut!(iter, enc);
                enc.serialize_array(depth, |enc, depth| {
                    if let Some(item) = iter.next(enc.vm)? {
                        enc.serialize_value(item, depth)?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })
            }),
            PyValue::Tuple(tuple) => self.with_entered_container(tuple.id(), |enc| {
                let iter = tuple.iter(enc.vm)?;
                defer_drop_mut!(iter, enc);
                enc.serialize_array(depth, |enc, depth| {
                    if let Some(item) = iter.next(enc.vm)? {
                        enc.serialize_value(item, depth)?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })
            }),
            PyValue::Dict(dict) => {
                // Dict pre-materializes entries because `sort_keys` and
                // `skipkeys` need to mutate the entries vector before output.
                let entries = self.collect_dict_entries(&dict);
                let this = self;
                defer_drop_mut!(entries, this);
                // Need to explicitly acquire a recursion token for the dict as we don't go
                // via the default dict iterator.
                let token = this.vm.recursion_token()?;
                defer_drop!(token, this);
                this.with_entered_container(dict.id(), |enc| enc.serialize_dict(entries, depth))
            }
            _ => {
                // CPython names the class by `__name__`: `Decimal`, not `decimal.Decimal`.
                // A user class's name is its `__name__` already, dots and all.
                let type_name = value.py_type_name(self.vm);
                let bare = if matches!(value.py_type(self.vm), Type::Instance(_)) {
                    &*type_name
                } else {
                    type_name.rsplit_once('.').map_or(&*type_name, |(_, bare)| bare)
                };
                Err(ExcType::json_not_serializable_error(bare))
            }
        }
    }

    /// Streams items from a list or tuple into a JSON array, formatting
    /// separators and indentation between calls to `write_next`.
    ///
    /// `write_next` is invoked once per item slot and should write the JSON
    /// for the next item, returning `Ok(true)` if it wrote one or `Ok(false)`
    /// when the underlying iterator is exhausted. This shape lets the caller
    /// plug in either a [`ListIter`](crate::types::list::ListIter) or a
    /// [`TupleIter`](crate::types::tuple::TupleIter) without having to unify
    /// their lending-iterator types.
    ///
    /// Circular-reference tracking still happens at the container level (via
    /// [`with_entered_container`](Self::with_entered_container)) before this
    /// helper is called; the recursion-depth bound is enforced by the
    /// iterator's `RecursionToken`.
    fn serialize_array(
        &mut self,
        depth: usize,
        mut write_next: impl FnMut(&mut Self, usize) -> RunResult<bool>,
    ) -> RunResult<()> {
        self.out.push('[');
        let pretty = self.config.indent.is_some();
        let mut wrote_any = false;
        loop {
            // Write the separator + indent BEFORE we know whether the next
            // item exists; if `write_next` reports exhaustion we roll the
            // cursor back.
            let prefix_start = self.out.len();
            if wrote_any {
                self.out.push_str(&self.config.item_separator);
            }
            if pretty {
                self.out.push('\n');
                write_indent(self.out, self.config, depth + 1);
            }
            let body_start = self.out.len();
            if !write_next(self, depth + 1)? {
                self.out.truncate(prefix_start);
                break;
            }
            debug_assert!(
                self.out.len() > body_start,
                "write_next reported true but wrote nothing"
            );
            wrote_any = true;
        }
        if pretty && wrote_any {
            self.out.push('\n');
            write_indent(self.out, self.config, depth);
        }
        self.out.push(']');
        Ok(())
    }

    /// Copies a dict's `(key, value)` pairs into an owned `Vec` via
    /// [`DictIter`](crate::types::dict::DictIter), so subsequent passes
    /// (`skipkeys`, `sort_keys`) can mutate the buffer in place. The
    /// `DictIter`'s recursion token is acquired and released during the
    /// copy, bounding the depth of the *enclosing* `serialize_value` call.
    fn collect_dict_entries(&mut self, dict: &HeapRead<'h, Dict>) -> Vec<(Value, Value)> {
        dict.get(self.vm.heap)
            .iter()
            .map(|(k, v)| (k.clone_with_heap(self.vm.heap), v.clone_with_heap(self.vm.heap)))
            .collect::<Vec<_>>()
    }

    /// Serializes a dict as a JSON object.
    ///
    /// Dict keys are validated and optionally skipped before serialization.
    /// When `sort_keys=True`, entries are sorted using Python comparison
    /// semantics on the original keys so mixed incomparable key types raise
    /// the same style of `TypeError` as CPython.
    fn serialize_dict(&mut self, entries: &mut Vec<(Value, Value)>, depth: usize) -> RunResult<()> {
        if self.config.skipkeys() {
            skip_disallowed_dict_keys(entries, self.vm);
        } else if let Some((key, _)) = entries.iter().find(|(key, _)| !is_json_key_allowed(key, self.vm)) {
            return Err(ExcType::json_invalid_key_error(&key.py_type_name(self.vm)));
        }

        if self.config.sort_keys() {
            sort_dict_entries(entries, self.vm)?;
        }

        self.out.push('{');

        let pretty = self.config.indent.is_some();
        for (index, (key, value)) in entries.iter().enumerate() {
            if index != 0 {
                self.out.push_str(&self.config.item_separator);
            }
            if pretty {
                self.out.push('\n');
                write_indent(self.out, self.config, depth + 1);
            }
            write_json_key(key, self.out, self.config, self.vm)?;
            self.out.push_str(&self.config.key_separator);
            self.serialize_value(value, depth + 1)?;
        }
        if pretty && !entries.is_empty() {
            self.out.push('\n');
            write_indent(self.out, self.config, depth);
        }
        self.out.push('}');
        Ok(())
    }

    /// Preflights the output buffer's next doubling before writing a value of
    /// about `value_len` bytes, so serializing a large document raises
    /// `MemoryError` instead of reallocating past the limit's headroom.
    /// Escaping that outgrows the margin is left to the allocator's hard limit.
    fn check_output_growth(&mut self, value_len: usize) -> RunResult<()> {
        const MARGIN: usize = 64;
        let (len, capacity) = (self.out.len(), self.out.capacity());
        let needed = len.saturating_add(value_len).saturating_add(MARGIN);
        if needed > capacity {
            // `String::reserve` grows to at least double the capacity.
            let target = needed.max(capacity.saturating_mul(2));
            self.vm.heap.tracker.check_allocation(target - capacity)?;
            self.out.reserve(needed - len);
        }
        Ok(())
    }

    /// Runs `f` while `heap_id` is marked active for cycle detection.
    ///
    /// Centralizes the push/pop bookkeeping so every serialization path pops
    /// the container again regardless of whether recursive serialization
    /// succeeds or returns early with an error.
    fn with_entered_container<T>(
        &mut self,
        heap_id: HeapId,
        f: impl FnOnce(&mut Self) -> RunResult<T>,
    ) -> RunResult<T> {
        if self.active_containers.contains(&heap_id) {
            return Err(ExcType::json_circular_reference_error());
        }
        self.active_containers.push(heap_id);
        let result = f(self);
        self.active_containers
            .pop()
            .expect("entered container missing from JSON serialization stack");
        result
    }
}

/// Sorts dict entries in-place using Python comparison semantics on the keys.
///
/// The implementation mirrors the error style used by `sorted()` and
/// `list.sort()`: when two keys are not orderable, it raises
/// `TypeError: '<' not supported between instances of ...`.
fn sort_dict_entries(entries: &mut Vec<(Value, Value)>, vm: &mut VM<'_>) -> RunResult<()> {
    let mut indices: Vec<usize> = (0..entries.len()).collect();
    let compare_values: Vec<Value> = entries.iter().map(|(key, _)| key.clone_with_heap(vm)).collect();
    defer_drop!(compare_values, vm);
    sort_indices(&mut indices, compare_values.as_slice(), false, vm)?;
    apply_permutation(entries.as_mut_slice(), &mut indices);
    Ok(())
}

/// Removes dict entries whose keys are not JSON-serializable, preserving order.
fn skip_disallowed_dict_keys(entries: &mut Vec<(Value, Value)>, vm: &mut VM<'_>) {
    // Use two pointers to preserve relative order
    let mut write = 0;
    for read in 0..entries.len() {
        if is_json_key_allowed(&entries[read].0, vm) {
            if write != read {
                entries.swap(write, read);
            }
            write += 1;
        }
    }

    // Drain the disallowed entries
    entries.drain(write..).drop_with(vm);
}

/// Returns whether a value is an allowed JSON object key type.
///
/// CPython accepts strings, integers, floats, booleans, and `None`, then
/// coerces the non-string cases to JSON strings during output.
fn is_json_key_allowed(value: &Value, vm: &VM<'_>) -> bool {
    matches!(
        value.py_value(vm),
        PyValue::None | PyValue::Bool(_) | PyValue::Int(_) | PyValue::Float(_) | PyValue::Str(_)
    )
}

/// Serializes a dict key by applying CPython's JSON key coercions.
///
/// Non-string supported key types are rendered to their JSON string form first,
/// then escaped as a JSON string token.
fn write_json_key(key: &Value, out: &mut String, config: &JsonDumpsConfig, vm: &VM<'_>) -> RunResult<()> {
    let ensure_ascii = config.ensure_ascii();
    match key.py_value(vm) {
        PyValue::None => write_json_ascii_key("null", out),
        PyValue::Bool(true) => write_json_ascii_key("true", out),
        PyValue::Bool(false) => write_json_ascii_key("false", out),
        PyValue::Int(integer) => {
            out.push('"');
            integer.write_decimal(out, vm)?;
            out.push('"');
        }
        PyValue::Float(value) => serialize_float_key(value, out, config)?,
        PyValue::Str(value) => write_json_string(value.as_str(vm), out, ensure_ascii),
        _ => return Err(ExcType::json_invalid_key_error(&key.py_type_name(vm))),
    }
    Ok(())
}

/// Writes an already-ASCII JSON object key without going through the string
/// escaper.
///
/// Coerced keys such as `None`, booleans, and numeric reprs are always ASCII
/// and require no escaping, so this avoids building intermediate `String`
/// values on the dict-key hot path.
fn write_json_ascii_key(value: &str, out: &mut String) {
    out.push('"');
    out.push_str(value);
    out.push('"');
}

/// Serializes a float value as a quoted JSON object key, respecting `allow_nan`.
///
/// Non-finite values (NaN, +/-Infinity) are emitted as their Python repr
/// (`NaN`, `Infinity`, `-Infinity`) when `allow_nan` is enabled. When disabled,
/// the same `ValueError` raised for non-finite float *values* applies to keys
/// too, matching CPython's behavior.
fn serialize_float_key(value: f64, out: &mut String, config: &JsonDumpsConfig) -> RunResult<()> {
    out.push('"');
    if value.is_nan() {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("nan"));
        }
        out.push_str("NaN");
    } else if value == f64::INFINITY {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("inf"));
        }
        out.push_str("Infinity");
    } else if value == f64::NEG_INFINITY {
        if !config.allow_nan() {
            return Err(ExcType::json_nan_error("-inf"));
        }
        out.push_str("-Infinity");
    } else {
        write_json_float_text(value, out);
    }
    out.push('"');
    Ok(())
}

/// Serializes a float using JSON's number and NaN rules.
///
/// Finite floats use CPython-compatible `json` float formatting, including the
/// switch to exponent notation for very small or very large magnitudes while
/// still preserving a decimal point for whole-valued non-exponent outputs.
fn serialize_float(value: f64, out: &mut String, config: &JsonDumpsConfig) -> RunResult<()> {
    if value.is_nan() {
        if config.allow_nan() {
            out.push_str("NaN");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("nan"))
        }
    } else if value == f64::INFINITY {
        if config.allow_nan() {
            out.push_str("Infinity");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("inf"))
        }
    } else if value == f64::NEG_INFINITY {
        if config.allow_nan() {
            out.push_str("-Infinity");
            Ok(())
        } else {
            Err(ExcType::json_nan_error("-inf"))
        }
    } else {
        write_json_float_text(value, out);
        Ok(())
    }
}

/// Writes a finite float using CPython-compatible JSON float repr rules.
///
/// Python switches to scientific notation when the magnitude is `>= 1e16` or
/// `< 1e-4` (and non-zero). We decide notation by comparing the absolute value
/// directly against these thresholds rather than using `log10().floor()`, which
/// has precision errors at boundary values (e.g. `9999999999999998.0` whose
/// `log10` rounds up to `16.0`). Direct comparison is exact because `1e16` is
/// exactly representable as f64 and `1e-4` as an f64 constant aligns with
/// Python's notation boundary.
///
/// Each path formats the float exactly once: the scientific path writes via
/// `{:e}` and post-processes the exponent to Python style (`e+XX` / `e-XX`),
/// while the fixed path uses `Display` with a `.0` suffix for whole numbers.
fn write_json_float_text(value: f64, out: &mut String) {
    let abs = value.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // Python-style scientific notation: format via `{:e}`, then rewrite the
        // exponent from Rust's bare `e<N>` to Python's `e+XX` / `e-XX`.
        let start = out.len();
        write!(out, "{value:e}").expect("writing to String cannot fail");
        let e_pos = out[start..].find('e').expect("scientific format must contain 'e'") + start;
        let exponent: i32 = out[e_pos + 1..].parse().expect("exponent must be a valid integer");
        out.truncate(e_pos);
        let exp_sign = if exponent >= 0 { '+' } else { '-' };
        write!(out, "e{exp_sign}{:02}", exponent.unsigned_abs()).expect("writing to String cannot fail");
    } else {
        // Fixed notation: single `Display` write, appending `.0` for whole numbers.
        let start = out.len();
        write!(out, "{value}").expect("writing to String cannot fail");
        if !out[start..].contains('.') {
            out.push_str(".0");
        }
    }
}

/// Writes indentation for pretty-printed JSON output.
///
/// The `indent` string is repeated once for each nesting level, matching
/// CPython's behavior for both numeric and string indentation.
fn write_indent(out: &mut String, config: &JsonDumpsConfig, depth: usize) {
    if let Some(indent) = &config.indent {
        for _ in 0..depth {
            out.push_str(indent);
        }
    }
}

/// Writes a Rust string as a JSON string token.
///
/// Uses a byte-oriented batch strategy inspired by serde_json: a 256-entry
/// lookup table classifies each byte in O(1), and contiguous runs of safe bytes
/// are flushed with a single `push_str` rather than character-by-character.
///
/// When `ensure_ascii` is enabled, non-ASCII code points (bytes >= 0x80) are
/// emitted as `\uXXXX` escapes using surrogate pairs for supplementary-plane
/// characters.
fn write_json_string(value: &str, out: &mut String, ensure_ascii: bool) {
    out.push('"');
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];

        if ensure_ascii && byte >= 0x7F {
            // Flush the safe ASCII run accumulated so far.
            out.push_str(&value[start..i]);
            if byte == 0x7F {
                // DEL (0x7F) is a control character that CPython escapes.
                out.push_str("\\u007f");
                i += 1;
            } else {
                // Decode the full character at this position and emit \uXXXX escapes.
                let ch = value[i..].chars().next().expect("valid UTF-8");
                write_json_escape_for_non_ascii(ch, out);
                i += ch.len_utf8();
            }
            start = i;
            continue;
        }

        let escape = ESCAPE_TABLE[byte as usize];
        if escape == 0 {
            // Safe byte — keep scanning.
            i += 1;
            continue;
        }

        // Flush the safe run before this byte.
        out.push_str(&value[start..i]);

        // Write the escape sequence.
        match escape {
            b'b' => out.push_str("\\b"),
            b't' => out.push_str("\\t"),
            b'n' => out.push_str("\\n"),
            b'f' => out.push_str("\\f"),
            b'r' => out.push_str("\\r"),
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'u' => {
                write!(out, "\\u{:04x}", u32::from(byte)).expect("writing to String cannot fail");
            }
            _ => unreachable!(),
        }

        i += 1;
        start = i;
    }

    // Flush the final safe run.
    out.push_str(&value[start..]);
    out.push('"');
}

/// Byte lookup table for JSON string escaping.
///
/// Each entry is either 0 (byte is safe, no escaping needed) or a shorthand
/// character that indicates which escape to emit:
/// - `b'"'`  → `\"`
/// - `b'\\'` → `\\`
/// - `b'b'`  → `\b` (backspace, 0x08)
/// - `b't'`  → `\t` (tab, 0x09)
/// - `b'n'`  → `\n` (newline, 0x0A)
/// - `b'f'`  → `\f` (form feed, 0x0C)
/// - `b'r'`  → `\r` (carriage return, 0x0D)
/// - `b'u'`  → `\u00XX` (other control characters, 0x00–0x1F)
#[rustfmt::skip]
static ESCAPE_TABLE: [u8; 256] = {
    let mut table = [0u8; 256];
    // Control characters 0x00–0x1F default to \u00XX escapes.
    let mut i = 0;
    while i < 0x20 {
        table[i] = b'u';
        i += 1;
    }
    // Override the named escapes.
    table[0x08] = b'b';  // backspace
    table[0x09] = b't';  // tab
    table[0x0A] = b'n';  // newline
    table[0x0C] = b'f';  // form feed
    table[0x0D] = b'r';  // carriage return
    table[0x22] = b'"';  // quote
    table[0x5C] = b'\\'; // backslash
    table
};

/// Writes a non-ASCII character using JSON `\uXXXX` escapes.
///
/// Code points above `U+FFFF` are encoded as UTF-16 surrogate pairs to match
/// CPython's `ensure_ascii=True` behavior.
fn write_json_escape_for_non_ascii(ch: char, out: &mut String) {
    let code = ch as u32;
    if code <= 0xFFFF {
        write!(out, "\\u{code:04x}").expect("writing to String cannot fail");
    } else {
        let code = code - 0x1_0000;
        let high = 0xD800 + (code >> 10);
        let low = 0xDC00 + (code & 0x3FF);
        write!(out, "\\u{high:04x}\\u{low:04x}").expect("writing to String cannot fail");
    }
}
