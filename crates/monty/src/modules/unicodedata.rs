//! Implementation of Python's `unicodedata` module.
//!
//! Provides access to the Unicode Character Database: general categories,
//! canonical combining classes, character names, and normalization. Only the
//! widely-used functions are implemented; see `limitations/unicodedata.md` for
//! the full list of divergences from CPython (notably the data-heavy functions
//! `decimal`/`digit`/`numeric`/`bidirectional`/`east_asian_width`/`mirrored`/
//! `decomposition`, which are intentionally not implemented).
//!
//! ## Implemented functions
//!
//! - `category(chr)` — two-letter general category (e.g. `"Lu"`, `"Nd"`)
//! - `name(chr[, default])` — the Unicode name of a character
//! - `lookup(name)` — the character with a given name
//! - `combining(chr)` — the canonical combining class as an int
//! - `normalize(form, unistr)` — NFC/NFD/NFKC/NFKD normalization
//! - `is_normalized(form, unistr)` — whether a string is already normalized
//!
//! ## Constants
//!
//! `unidata_version` — the Unicode version string.
//!
//! All functions are pure computations that don't require host involvement, so
//! they return `Value` directly (wrapped in `CallResult::Value` by the dispatch
//! in [`super`]).

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    string_builder::StringBuilder,
    types::{Module, str::allocate_string},
    value::Value,
};

/// The Unicode version reported by `unicodedata.unidata_version`.
///
/// Hard-coded to match CPython 3.14 rather than derived from the backing
/// crates: those crates' data tables may lag or lead this version (e.g.
/// `unicode-normalization` currently ships Unicode 17.0 tables), so reporting
/// their versions would diverge from CPython. See `limitations/unicodedata.md`.
const UNIDATA_VERSION: &str = "16.0.0";

/// `unicodedata` module functions — each variant corresponds to a Python-visible function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum UnicodedataFunctions {
    Category,
    Name,
    Lookup,
    Combining,
    Normalize,
    IsNormalized,
}

/// Static mapping of attribute names to functions for module creation.
const UNICODEDATA_FUNCTIONS: &[(StaticStrings, UnicodedataFunctions)] = &[
    (StaticStrings::Category, UnicodedataFunctions::Category),
    (StaticStrings::Name, UnicodedataFunctions::Name),
    (StaticStrings::Lookup, UnicodedataFunctions::Lookup),
    (StaticStrings::Combining, UnicodedataFunctions::Combining),
    (StaticStrings::Normalize, UnicodedataFunctions::Normalize),
    (StaticStrings::IsNormalized, UnicodedataFunctions::IsNormalized),
];

/// Creates the `unicodedata` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Unicodedata);

    for (name, func) in UNICODEDATA_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Unicodedata(*func)), vm);
    }

    let version = allocate_string(UNIDATA_VERSION, vm.heap)?;
    module.set_attr(StaticStrings::UnidataVersion, version, vm);

    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to a `unicodedata` module function.
///
/// All functions are pure computations and return `Value` directly.
pub(super) fn call(
    vm: &mut VM<'_, impl ResourceTracker>,
    function: UnicodedataFunctions,
    args: ArgValues,
) -> RunResult<Value> {
    match function {
        UnicodedataFunctions::Category => uni_category(vm, args),
        UnicodedataFunctions::Name => uni_name(vm, args),
        UnicodedataFunctions::Lookup => uni_lookup(vm, args),
        UnicodedataFunctions::Combining => uni_combining(vm, args),
        UnicodedataFunctions::Normalize => uni_normalize(vm, args),
        UnicodedataFunctions::IsNormalized => uni_is_normalized(vm, args),
    }
}

/// `unicodedata.category(chr)` — the two-letter general category abbreviation.
///
/// Returns e.g. `"Lu"` (uppercase letter), `"Nd"` (decimal digit), or `"Cn"`
/// for unassigned code points.
fn uni_category(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("category", vm.heap)?;
    defer_drop!(value, vm);
    let c = single_char(value, "category", None, vm)?;
    Ok(allocate_string(category_abbrev(get_general_category(c)), vm.heap)?)
}

/// `unicodedata.name(chr[, default])` — the Unicode name of a character.
///
/// Raises `ValueError("no such name")` when the character has no name and no
/// `default` is supplied; otherwise returns `default`.
fn uni_name(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let NameArgs {
        chr: chr_val,
        default: default_val,
    } = NameArgs::from_args(args, vm)?;
    defer_drop!(chr_val, vm);

    // `default_val` is not covered by `defer_drop!` (it's an `Option` we may
    // return by value), so drop it explicitly on every path that doesn't.
    let c = match single_char(chr_val, "name", Some(1), vm) {
        Ok(c) => c,
        Err(e) => {
            if let Some(d) = default_val {
                d.drop_with(vm);
            }
            return Err(e);
        }
    };

    match unicode_names2::name(c) {
        Some(name) => {
            if let Some(d) = default_val {
                d.drop_with(vm);
            }
            Ok(allocate_string(name.to_string(), vm.heap)?)
        }
        None => match default_val {
            Some(d) => Ok(d),
            None => Err(SimpleException::new_msg(ExcType::ValueError, "no such name").into()),
        },
    }
}

/// `unicodedata.lookup(name)` — the character with a given Unicode name.
///
/// Raises `KeyError("undefined character name '<name>'")` when no character has
/// that name. Unlike CPython, named sequences are not resolved.
fn uni_lookup(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("lookup", vm.heap)?;
    defer_drop!(value, vm);
    let name = value.to_str(vm)?;
    match unicode_names2::character(name) {
        Some(c) => Ok(allocate_string(c.to_string(), vm.heap)?),
        None => Err(SimpleException::new_msg(ExcType::KeyError, format!("undefined character name '{name}'")).into()),
    }
}

/// `unicodedata.combining(chr)` — the canonical combining class as an int.
///
/// Returns `0` for characters with no combining class (the common case).
fn uni_combining(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("combining", vm.heap)?;
    defer_drop!(value, vm);
    let c = single_char(value, "combining", None, vm)?;
    Ok(Value::Int(i64::from(canonical_combining_class(c))))
}

/// `unicodedata.normalize(form, unistr)` — normalize a string to NFC/NFD/NFKC/NFKD.
fn uni_normalize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let NormalizeArgs { form, unistr } = NormalizeArgs::from_args(args, vm)?;
    defer_drop!(form, vm);
    defer_drop!(unistr, vm);
    let form = NormForm::parse(form.as_str(vm))?;
    normalize_with(form, unistr.as_str(vm), vm.heap)
}

/// `unicodedata.is_normalized(form, unistr)` — whether a string is already normalized.
fn uni_is_normalized(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let IsNormalizedArgs { form, unistr } = IsNormalizedArgs::from_args(args, vm)?;
    defer_drop!(form, vm);
    defer_drop!(unistr, vm);
    let normalized = match NormForm::parse(form.as_str(vm))? {
        NormForm::Nfc => unicode_normalization::is_nfc(unistr.as_str(vm)),
        NormForm::Nfd => unicode_normalization::is_nfd(unistr.as_str(vm)),
        NormForm::Nfkc => unicode_normalization::is_nfkc(unistr.as_str(vm)),
        NormForm::Nfkd => unicode_normalization::is_nfkd(unistr.as_str(vm)),
    };
    Ok(Value::Bool(normalized))
}

/// Argument shape for `name(chr, default=..., /)`.
///
/// Both arguments are positional-only (matching CPython's C signature). `chr`
/// stays a raw `Value` because it needs the bespoke single-character coercion
/// in [`single_char`] (CPython's "must be a unicode character" wording has no
/// `FromArgs` equivalent); `default` is any object returned verbatim when the
/// character has no name. `style = unpack` gives the `PyArg_UnpackTuple` arity
/// wording CPython uses here (`name expected at most 2 arguments, got 3`).
#[derive(FromArgs)]
#[from_args(name = "name", style = unpack, kwarg_error_name = "unicodedata.name")]
struct NameArgs {
    #[from_args(pos_only)]
    chr: Value,
    #[from_args(pos_only, default)]
    default: Option<Value>,
}

/// Argument shape for `normalize(form, unistr, /)`.
///
/// Both arguments are positional-only, zero-copy [`StrArg`]s: `bad_arg` gives
/// the exact `normalize() argument N must be str, not <type>` type error for
/// both. The form's *value* is validated by [`NormForm::parse`] in the body —
/// CPython type-checks every argument before rejecting an unknown form name,
/// so `normalize('XYZ', 123)` must raise the arg-2 `TypeError`, not the
/// `ValueError`. `style = unpack` (with min == max here) reproduces CPython's
/// `normalize expected 2 arguments, got N` arity error.
#[derive(FromArgs)]
#[from_args(name = "normalize", style = unpack, bad_arg, kwarg_error_name = "unicodedata.normalize")]
struct NormalizeArgs {
    #[from_args(pos_only)]
    form: StrArg,
    #[from_args(pos_only)]
    unistr: StrArg,
}

/// Argument shape for `is_normalized(form, unistr, /)` — see [`NormalizeArgs`].
#[derive(FromArgs)]
#[from_args(name = "is_normalized", style = unpack, bad_arg, kwarg_error_name = "unicodedata.is_normalized")]
struct IsNormalizedArgs {
    #[from_args(pos_only)]
    form: StrArg,
    #[from_args(pos_only)]
    unistr: StrArg,
}

/// The four Unicode normalization forms accepted by `normalize`/`is_normalized`.
#[derive(Clone, Copy)]
enum NormForm {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

impl NormForm {
    /// Maps a form name to its variant; unknown names raise CPython's
    /// `ValueError: invalid normalization form`. Called from the function
    /// body — after `FromArgs` has type-checked every argument — because
    /// CPython validates the form's *value* only once binding succeeds.
    fn parse(name: &str) -> RunResult<Self> {
        match name {
            "NFC" => Ok(Self::Nfc),
            "NFD" => Ok(Self::Nfd),
            "NFKC" => Ok(Self::Nfkc),
            "NFKD" => Ok(Self::Nfkd),
            _ => Err(SimpleException::new_msg(ExcType::ValueError, "invalid normalization form").into()),
        }
    }
}

/// Normalizes `text` into a freshly allocated Python string.
///
/// The output length is not bounded by the input (decomposition can expand a
/// single code point into several), so the result is built through
/// [`StringBuilder`] which reserves bytes with the resource tracker as it grows.
fn normalize_with(form: NormForm, text: &str, heap: &Heap<impl ResourceTracker>) -> RunResult<Value> {
    let mut builder = StringBuilder::new(heap.tracker());
    match form {
        NormForm::Nfc => {
            for c in text.nfc() {
                builder.push(c)?;
            }
        }
        NormForm::Nfd => {
            for c in text.nfd() {
                builder.push(c)?;
            }
        }
        NormForm::Nfkc => {
            for c in text.nfkc() {
                builder.push(c)?;
            }
        }
        NormForm::Nfkd => {
            for c in text.nfkd() {
                builder.push(c)?;
            }
        }
    }
    builder.finish(heap)
}

/// Extracts a single `char` from a value that must be a one-character string.
///
/// Matches CPython's two distinct error shapes: a non-`str` argument yields
/// `"<fn>() argument[ N] must be a unicode character, not <type>"`, while a
/// string of the wrong length yields `"<fn>(): argument[ N] must be a unicode
/// character, not a string of length <n>"`. `arg_num` supplies the ` N` suffix
/// (only `name()` numbers its argument).
fn single_char(
    value: &Value,
    fn_name: &str,
    arg_num: Option<u32>,
    vm: &VM<'_, impl ResourceTracker>,
) -> RunResult<char> {
    let arg_word = match arg_num {
        Some(n) => format!("argument {n}"),
        None => "argument".to_string(),
    };
    if !value.is_str(vm.heap) {
        return Err(ExcType::type_error(format!(
            "{fn_name}() {arg_word} must be a unicode character, not {}",
            value.py_type_name(vm)
        )));
    }
    let s = value.to_str(vm)?;
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(c),
        _ => Err(ExcType::type_error(format!(
            "{fn_name}(): {arg_word} must be a unicode character, not a string of length {}",
            s.chars().count()
        ))),
    }
}

/// Maps a [`GeneralCategory`] to its two-letter Unicode abbreviation.
///
/// These abbreviations are the values CPython's `unicodedata.category` returns.
fn category_abbrev(category: GeneralCategory) -> &'static str {
    match category {
        GeneralCategory::UppercaseLetter => "Lu",
        GeneralCategory::LowercaseLetter => "Ll",
        GeneralCategory::TitlecaseLetter => "Lt",
        GeneralCategory::ModifierLetter => "Lm",
        GeneralCategory::OtherLetter => "Lo",
        GeneralCategory::NonspacingMark => "Mn",
        GeneralCategory::SpacingMark => "Mc",
        GeneralCategory::EnclosingMark => "Me",
        GeneralCategory::DecimalNumber => "Nd",
        GeneralCategory::LetterNumber => "Nl",
        GeneralCategory::OtherNumber => "No",
        GeneralCategory::ConnectorPunctuation => "Pc",
        GeneralCategory::DashPunctuation => "Pd",
        GeneralCategory::OpenPunctuation => "Ps",
        GeneralCategory::ClosePunctuation => "Pe",
        GeneralCategory::InitialPunctuation => "Pi",
        GeneralCategory::FinalPunctuation => "Pf",
        GeneralCategory::OtherPunctuation => "Po",
        GeneralCategory::MathSymbol => "Sm",
        GeneralCategory::CurrencySymbol => "Sc",
        GeneralCategory::ModifierSymbol => "Sk",
        GeneralCategory::OtherSymbol => "So",
        GeneralCategory::SpaceSeparator => "Zs",
        GeneralCategory::LineSeparator => "Zl",
        GeneralCategory::ParagraphSeparator => "Zp",
        GeneralCategory::Control => "Cc",
        GeneralCategory::Format => "Cf",
        GeneralCategory::Surrogate => "Cs",
        GeneralCategory::PrivateUse => "Co",
        GeneralCategory::Unassigned => "Cn",
        // `GeneralCategory` is `#[non_exhaustive]`. Every category in the Unicode
        // standard is enumerated above, so this arm is unreachable today; it only
        // guards against the crate adding a variant in a future Unicode revision.
        _ => "Cn",
    }
}
