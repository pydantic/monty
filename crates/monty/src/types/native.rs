//! Native class registry and the runtime table adapted from [`ClassDef`] implementations.
//! Each owning module defines its class metadata, constructor and members.
//! [`native_types!`] only associates stable [`Type`] variants with those definitions.
//!
//! Variant names are the dump contract (dumps store them as strings).
//! [`Type::legacy_index`] pins numeric identity codes independently of table order;
//! existing codes must never change or be reused.

use std::{fmt, iter::FusedIterator, slice, sync::LazyLock};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
};
use strum::VariantNames;

use crate::{
    args::ArgValues,
    asyncio, builtins,
    bytecode::{CallResult, VM},
    exception_private::{self, ExcType, RunResult},
    function, heap_data, modules,
    py_attrs::{AttrTable, SubtypeCall},
    types::{self, class_def::ClassDef},
    value,
};

/// Facts about one native type, read through [`Type::def`].
///
/// Adapted from [`ClassDef`] at compile time; runtime dispatch needs no trait objects.
#[expect(clippy::struct_excessive_bools, reason = "independent facts, each read on its own")]
pub(crate) struct TypeDef {
    /// CPython's `tp_name`: qualified for module types (`datetime.date`), so it
    /// is what reprs and error messages print; `__name__` strips the module.
    /// Types hash by this string, so it is part of the dump contract.
    pub name: &'static str,
    /// The name bound in the builtins namespace, if the type is one (`iter` for `Iterator`).
    pub builtin: Option<&'static str>,
    /// Direct bases in declaration order; empty only for `object`.
    pub bases: &'static [Type],
    /// The C3 linearisation, written only for types with several bases;
    /// [`Type::mro`] walks the first base otherwise.
    pub mro: Option<&'static [Type]>,
    /// Whether CPython allows subclassing (`Py_TPFLAGS_BASETYPE`).
    pub basetype: bool,
    /// Whether subscripting the type builds a `types.GenericAlias` (`list[int]`).
    pub class_getitem: bool,
    /// Whether multiplication can fall back to sequence repetition.
    pub sequence_repeat: bool,
    /// Whether this is one of CPython's concrete iterator types.
    pub iterator: bool,
    /// How calling the type constructs an instance.
    pub construct: for<'h> fn(&mut VM<'h>, ArgValues) -> RunResult<CallResult>,
    pub attrs: Option<AttrTable>,
    pub subtype_call: Option<SubtypeCall>,
    /// The boundary exception type, for exception classes.
    pub exc: Option<ExcType>,
}

impl TypeDef {
    /// Adapts one class definition to the runtime's indexed metadata and function pointers.
    const fn of<T: ClassDef>() -> Self {
        Self {
            name: T::NAME,
            builtin: T::BUILTIN_NAME,
            bases: T::BASES,
            mro: T::MRO,
            basetype: T::BASETYPE,
            class_getitem: T::CLASS_GETITEM,
            sequence_repeat: T::SEQUENCE_REPEAT,
            iterator: T::ITERATOR,
            construct: T::construct,
            attrs: T::ATTRS,
            subtype_call: T::SUBTYPE_CALL,
            exc: T::EXC_TYPE,
        }
    }
}

/// Generates [`Type`], its [`TypeDef`] table and the name lookups from one list.
///
/// Each entry maps a stable enum variant to its owning [`ClassDef`] implementation.
macro_rules! native_types {
    ($(
        $(#[doc = $doc:literal])*
        $variant:ident => $definition:ty
    ),* $(,)?) => {
        /// A Rust-defined Python type: builtins, stdlib types and exception classes.
        ///
        /// Fieldless, so it is `Copy`, one byte and safe to store anywhere; a
        /// user-defined class is a heap object instead, and code that handles
        /// either uses [`TypeRef`](crate::types::TypeRef). Facts about a type
        /// come from [`Type::def`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[expect(clippy::enum_variant_names, reason = "`Type`, `NoneType` and friends mirror Python's names")]
        pub enum Type {
            $($(#[doc = $doc])* $variant,)*
        }

        impl Type {
            /// Every native type, in declaration order.
            pub(crate) const ALL: [Self; [$(stringify!($variant)),*].len()] = [$(Self::$variant),*];

            /// The Rust variant names, which dumps store.
            const VARIANT_NAMES: [&'static str; Self::ALL.len()] = [$(stringify!($variant)),*];
        }

        /// One entry per [`Type`], indexed by discriminant; a `const` so other
        /// tables can be computed from it at compile time.
        const TYPE_DEFS_CONST: [TypeDef; Type::ALL.len()] = [
            $(TypeDef::of::<$definition>(),)*
        ];

        /// The runtime copy of [`TYPE_DEFS_CONST`], read by [`Type::def`].
        static TYPE_DEFS: [TypeDef; Type::ALL.len()] = TYPE_DEFS_CONST;
    };
}

impl Type {
    /// The facts about this type.
    #[inline]
    pub(crate) fn def(self) -> &'static TypeDef {
        &TYPE_DEFS[self as usize]
    }

    /// CPython's `tp_name` (`int`, `datetime.date`, `json.JSONDecodeError`).
    #[inline]
    pub(crate) fn name(self) -> &'static str {
        self.def().name
    }

    /// The boundary exception type, if this is an exception class.
    #[inline]
    pub(crate) fn exc_type(self) -> Option<ExcType> {
        self.def().exc
    }

    /// The Rust variant name, which dumps store.
    pub(crate) fn variant_name(self) -> &'static str {
        Self::VARIANT_NAMES[self as usize]
    }

    /// Resolves a stored variant name; `None` for an unknown name.
    pub(crate) fn from_variant_name(name: &str) -> Option<Self> {
        Self::VARIANT_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Self::ALL[index])
    }

    /// Resolves a bare name in the builtins namespace (`int`, `ValueError`, `iter`).
    ///
    /// Module types such as `datetime.date` or `FrozenInstanceError` need an import.
    #[must_use]
    pub fn from_builtin_name(name: &str) -> Option<Self> {
        BUILTIN_TYPES.get(name)
    }

    /// The name this type is bound to in the builtins namespace, if any.
    #[must_use]
    pub fn builtin_name(self) -> Option<&'static str> {
        self.def().builtin
    }

    /// Permanent native type codes, independent of the table's dense discriminants.
    /// Slots 27–29 remain reserved for the former `HostClass`, `Instance` and `Exception` variants.
    pub(crate) fn legacy_index(self) -> u32 {
        match self {
            Self::Ellipsis => 0,
            Self::NotImplementedType => 1,
            Self::Type => 2,
            Self::NoneType => 3,
            Self::Bool => 4,
            Self::Int => 5,
            Self::Float => 6,
            Self::Complex => 7,
            Self::Range => 8,
            Self::Slice => 9,
            Self::Date => 10,
            Self::DateTime => 11,
            Self::TimeDelta => 12,
            Self::TimeZone => 13,
            Self::Str => 14,
            Self::Bytes => 15,
            Self::List => 16,
            Self::Tuple => 17,
            Self::NamedTuple => 18,
            Self::Dict => 19,
            Self::DefaultDict => 20,
            Self::Counter => 21,
            Self::DictKeys => 22,
            Self::DictItems => 23,
            Self::DictValues => 24,
            Self::Set => 25,
            Self::FrozenSet => 26,
            Self::Function => 30,
            Self::BuiltinFunction => 31,
            Self::Cell => 32,
            Self::Iterator => 33,
            Self::ListIterator => 34,
            Self::CallableIterator => 35,
            Self::Coroutine => 36,
            Self::Module => 37,
            Self::TextIOWrapper => 38,
            Self::BufferedReader => 39,
            Self::BufferedWriter => 40,
            Self::BufferedRandom => 41,
            Self::SpecialForm => 42,
            Self::Path => 43,
            Self::Property => 44,
            Self::RePattern => 45,
            Self::ReMatch => 46,
            Self::DirEntry => 47,
            Self::ScandirIterator => 48,
            Self::WalkGenerator => 49,
            Self::TupleIterator => 50,
            Self::StrAsciiIterator => 51,
            Self::StrIterator => 52,
            Self::BytesIterator => 53,
            Self::RangeIterator => 54,
            Self::DictKeyIterator => 55,
            Self::DictItemIterator => 56,
            Self::DictValueIterator => 57,
            Self::SetIterator => 58,
            Self::ItertoolsCount => 59,
            Self::ItertoolsRepeat => 60,
            Self::DataclassField => 61,
            Self::Deque => 62,
            Self::DequeIterator => 63,
            Self::ItertoolsPairwise => 64,
            Self::ItertoolsCompress => 65,
            Self::ItertoolsIslice => 66,
            Self::ItertoolsChain => 67,
            Self::ItertoolsCycle => 68,
            Self::DataclassParams => 69,
            Self::MissingType => 70,
            Self::ItertoolsTakeWhile => 71,
            Self::ItertoolsDropWhile => 72,
            Self::ItertoolsFilterFalse => 73,
            Self::ItertoolsStarMap => 74,
            Self::Object => 75,
            Self::Time => 76,
            Self::Partial => 77,
            Self::ItertoolsAccumulate => 78,
            Self::ItertoolsBatched => 79,
            Self::ItertoolsZipLongest => 80,
            Self::GenericAlias => 81,
            Self::Union => 82,
            Self::ItertoolsCombinations => 83,
            Self::ItertoolsCombinationsWithReplacement => 84,
            Self::ItertoolsPermutations => 85,
            Self::ItertoolsProduct => 86,
            Self::ItertoolsGroupBy => 87,
            Self::ItertoolsGrouper => 88,
            Self::ItertoolsTee => 89,
            Self::ItertoolsTeeDataObject => 90,
            Self::Random => 91,
            Self::LruCacheWrapper => 92,
            Self::HashlibHash => 93,
            Self::HashlibHashXof => 94,
            Self::Blake2b => 95,
            Self::Blake2s => 96,
            Self::CsvReader => 97,
            Self::CsvWriter => 98,
            Self::CsvDictReader => 99,
            Self::CsvDictWriter => 100,
            Self::CsvDialect => 101,
            Self::Decimal => 102,
            Self::DecimalContext => 103,
            Self::DecimalSignalDict => 104,
            Self::DecimalContextManager => 105,
            Self::ForwardRef => 106,
            Self::Exception => 107,
            Self::BaseException => 108,
            Self::SystemExit => 109,
            Self::KeyboardInterrupt => 110,
            Self::ArithmeticError => 111,
            Self::OverflowError => 112,
            Self::ZeroDivisionError => 113,
            Self::LookupError => 114,
            Self::IndexError => 115,
            Self::KeyError => 116,
            Self::RuntimeError => 117,
            Self::NotImplementedError => 118,
            Self::RecursionError => 119,
            Self::AttributeError => 120,
            Self::FrozenInstanceError => 121,
            Self::NameError => 122,
            Self::UnboundLocalError => 123,
            Self::ValueError => 124,
            Self::UnicodeDecodeError => 125,
            Self::UnicodeEncodeError => 126,
            Self::JsonDecodeError => 127,
            Self::TomlDecodeError => 128,
            Self::ImportError => 129,
            Self::ModuleNotFoundError => 130,
            Self::OSError => 131,
            Self::FileNotFoundError => 132,
            Self::FileExistsError => 133,
            Self::IsADirectoryError => 134,
            Self::NotADirectoryError => 135,
            Self::PermissionError => 136,
            Self::UnsupportedOperation => 137,
            Self::TimeoutError => 138,
            Self::AssertionError => 139,
            Self::MemoryError => 140,
            Self::StopIteration => 141,
            Self::SyntaxError => 142,
            Self::TypeError => 143,
            Self::RePatternError => 144,
            Self::BinasciiError => 145,
            Self::BinasciiIncomplete => 146,
            Self::CsvError => 147,
            Self::DecimalException => 148,
            Self::DecimalClamped => 149,
            Self::DecimalInvalidOperation => 150,
            Self::DecimalConversionSyntax => 151,
            Self::DecimalDivisionByZero => 152,
            Self::DecimalDivisionImpossible => 153,
            Self::DecimalDivisionUndefined => 154,
            Self::DecimalInexact => 155,
            Self::DecimalInvalidContext => 156,
            Self::DecimalRounded => 157,
            Self::DecimalSubnormal => 158,
            Self::DecimalOverflow => 159,
            Self::DecimalUnderflow => 160,
            Self::DecimalFloatOperation => 161,
            Self::StatisticsError => 162,
            Self::Map => 163,
            Self::Filter => 164,
            Self::Task => 165,
            Self::InvalidStateError => 166,
            Self::StringIo => 167,
            Self::BytesIo => 168,
            Self::BlockingIOError => 169,
            Self::ClassMethod => 170,
            Self::StaticMethod => 171,
        }
    }

    /// This type then its ancestors, in method resolution order, ending at `object`.
    pub(crate) fn mro(self) -> NativeMro {
        match self.def().mro {
            Some(written) => NativeMro::Written(written.iter()),
            None => NativeMro::Chain(Some(self)),
        }
    }

    /// Whether `self` is `other` or inherits from it.
    #[inline]
    pub(crate) fn is_subclass_of(self, other: Self) -> bool {
        self == other || other == Self::Object || self.mro().skip(1).any(|ty| ty == other)
    }
}

/// Builtin-namespace names to their types, sorted by name for binary search.
///
/// Global loads of `range`, `list` and the like resolve through this on every
/// miss in the module globals. Stored inline rather than in a heap map, so the
/// first lookup inside a session allocates nothing that `max_memory` would count.
static BUILTIN_TYPES: LazyLock<BuiltinTypeTable> = LazyLock::new(|| {
    let mut table = BuiltinTypeTable {
        entries: [("", Type::Object); Type::ALL.len()],
        len: 0,
    };
    for ty in Type::ALL {
        if let Some(name) = ty.builtin_name() {
            table.entries[table.len] = (name, ty);
            table.len += 1;
        }
    }
    table.entries[..table.len].sort_unstable_by_key(|&(name, _)| name);
    table
});

/// Fixed-capacity storage for [`BUILTIN_TYPES`]; only the first `len` entries are used.
struct BuiltinTypeTable {
    entries: [(&'static str, Type); Type::ALL.len()],
    len: usize,
}

impl BuiltinTypeTable {
    /// Binary-searches the populated, name-sorted entries.
    fn get(&self, name: &str) -> Option<Type> {
        let entries = &self.entries[..self.len];
        entries
            .binary_search_by_key(&name, |&(entry, _)| entry)
            .ok()
            .map(|index| entries[index].1)
    }
}

/// Iterator over a native type's MRO, from [`Type::mro`].
pub(crate) enum NativeMro {
    /// A type with several bases: its written C3 linearisation.
    Written(slice::Iter<'static, Type>),
    /// A type with one base: walk the first base until `object`.
    Chain(Option<Type>),
}

impl Iterator for NativeMro {
    type Item = Type;

    fn next(&mut self) -> Option<Type> {
        match self {
            Self::Written(iter) => iter.next().copied(),
            Self::Chain(current) => {
                let ty = (*current)?;
                *current = ty.def().bases.first().copied();
                Some(ty)
            }
        }
    }
}

impl FusedIterator for NativeMro {}

impl From<ExcType> for Type {
    #[inline]
    fn from(exc: ExcType) -> Self {
        EXC_TYPES[exc as usize]
    }
}

/// The [`Type`] of every [`ExcType`], indexed by discriminant; computed from the table.
static EXC_TYPES: [Type; ExcType::VARIANTS.len()] = {
    let mut out = [Type::Object; ExcType::VARIANTS.len()];
    let mut i = 0;
    while i < Type::ALL.len() {
        if let Some(exc) = TYPE_DEFS_CONST[i].exc {
            out[exc as usize] = Type::ALL[i];
        }
        i += 1;
    }
    out
};

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Written as the variant name, a unit variant whose index is [`Type::legacy_index`].
impl Serialize for Type {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit_variant("Type", self.legacy_index(), self.variant_name())
    }
}

/// Reads a variant name, or the `{"Exception": "<ExcType>"}` map older dumps wrote
/// for exception classes before they joined the table.
impl<'de> Deserialize<'de> for Type {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TypeVisitor)
    }
}

/// Visitor for [`Type`]'s two stored shapes.
struct TypeVisitor;

impl<'de> Visitor<'de> for TypeVisitor {
    type Value = Type;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a type variant name or a legacy exception map")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Type, E> {
        Type::from_variant_name(name).ok_or_else(|| E::unknown_variant(name, &Type::VARIANT_NAMES))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Type, A::Error> {
        let Some(key) = map.next_key::<String>()? else {
            return Err(de::Error::invalid_length(0, &self));
        };
        if key != "Exception" {
            return Err(de::Error::unknown_variant(&key, &["Exception"]));
        }
        let exc: ExcType = map.next_value()?;
        Ok(Type::from(exc))
    }
}

native_types! {
    Ellipsis => value::EllipsisClass,
    NotImplementedType => value::NotImplementedTypeClass,
    Type => builtins::type_::TypeClass,
    NoneType => value::NoneTypeClass,
    Bool => types::r#type::BoolClass,
    Int => types::r#type::IntClass,
    Float => types::r#type::FloatClass,
    Complex => types::complex::Complex,
    Range => types::range::Range,
    Slice => types::slice::Slice,
    Date => types::date::Date,
    DateTime => types::datetime::DateTime,
    TimeDelta => types::timedelta::TimeDelta,
    TimeZone => types::timezone::TimeZone,
    Str => types::str::HeapStr,
    Bytes => types::bytes::Bytes,
    List => types::list::List,
    Tuple => types::tuple::Tuple,
    NamedTuple => types::namedtuple::NamedTuple,
    Dict => types::dict::Dict,
    DefaultDict => modules::collections::DefaultDictClass,
    Counter => modules::collections::CounterClass,
    DictKeys => types::dict_view::DictKeysView,
    DictItems => types::dict_view::DictItemsView,
    DictValues => types::dict_view::DictValuesView,
    Set => types::set::Set,
    FrozenSet => types::set::FrozenSet,
    Function => function::Function,
    BuiltinFunction => builtins::BuiltinFunctionClass,
    Cell => heap_data::CellValue,
    Iterator => builtins::iter::IteratorClass,
    ListIterator => types::list::ListIterator,
    CallableIterator => types::callable_iterator::CallableIterator,
    Coroutine => asyncio::Coroutine,
    Module => types::module::Module,
    TextIOWrapper => types::file::TextIOWrapperClass,
    BufferedReader => types::file::BufferedReaderClass,
    BufferedWriter => types::file::BufferedWriterClass,
    BufferedRandom => types::file::BufferedRandomClass,
    StringIo => types::io::StringIo,
    BytesIo => types::io::BytesIo,
    SpecialForm => modules::typing::SpecialFormClass,
    Path => types::path::Path,
    Property => types::builtin_descriptor::PropertyClass,
    RePattern => types::re_pattern::RePattern,
    ReMatch => types::re_match::ReMatch,
    DirEntry => types::dir_scan::DirEntry,
    ScandirIterator => types::dir_scan::ScandirIterator,
    WalkGenerator => types::dir_scan::WalkIterator,
    TupleIterator => types::tuple::TupleIterator,
    StrAsciiIterator => types::str::StrAsciiIteratorClass,
    StrIterator => types::str::StrIteratorClass,
    BytesIterator => types::bytes::BytesIterator,
    RangeIterator => types::range::RangeIterator,
    DictKeyIterator => types::dict::DictKeyIterator,
    DictItemIterator => types::dict::DictItemIterator,
    DictValueIterator => types::dict::DictValueIterator,
    SetIterator => types::set::SetIterator,
    ItertoolsCount => types::itertools::Count,
    ItertoolsRepeat => types::itertools::Repeat,
    DataclassField => modules::dataclasses::DataclassField,
    Deque => types::deque::Deque,
    DequeIterator => types::deque::DequeIterator,
    ItertoolsPairwise => types::itertools::Pairwise,
    ItertoolsCompress => types::itertools::Compress,
    ItertoolsIslice => types::itertools::Islice,
    ItertoolsChain => types::itertools::Chain,
    ItertoolsCycle => types::itertools::Cycle,
    DataclassParams => modules::dataclasses::DataclassParams,
    MissingType => modules::dataclasses::MissingTypeClass,
    ItertoolsTakeWhile => types::itertools::TakeWhile,
    ItertoolsDropWhile => types::itertools::DropWhile,
    ItertoolsFilterFalse => types::itertools::FilterFalse,
    ItertoolsStarMap => types::itertools::StarMap,
    Object => builtins::object::ObjectClass,
    Time => types::time::Time,
    Partial => types::partial::Partial,
    ItertoolsAccumulate => types::itertools::Accumulate,
    ItertoolsBatched => types::itertools::Batched,
    ItertoolsZipLongest => types::itertools::ZipLongest,
    GenericAlias => types::generic_alias::GenericAlias,
    Union => types::union::Union,
    ItertoolsCombinations => types::itertools::Combinations,
    ItertoolsCombinationsWithReplacement => modules::itertools::CombinationsWithReplacementClass,
    ItertoolsPermutations => types::itertools::Permutations,
    ItertoolsProduct => types::itertools::Product,
    ItertoolsGroupBy => types::itertools::GroupBy,
    ItertoolsGrouper => types::itertools::Grouper,
    ItertoolsTee => types::itertools::Tee,
    ItertoolsTeeDataObject => types::itertools::TeeBlock,
    Random => types::random::Random,
    LruCacheWrapper => types::lru_cache::LruCache,
    HashlibHash => modules::hashlib::HashlibHashClass,
    HashlibHashXof => modules::hashlib::HashlibHashXofClass,
    Blake2b => modules::hashlib::Blake2bClass,
    Blake2s => modules::hashlib::Blake2sClass,
    CsvReader => types::csv::Reader,
    CsvWriter => types::csv::Writer,
    CsvDictReader => types::csv::DictReader,
    CsvDictWriter => types::csv::DictWriter,
    CsvDialect => types::csv::Dialect,
    Decimal => types::decimal::Decimal,
    DecimalContext => types::decimal::DecContext,
    DecimalSignalDict => types::decimal::py_context::SignalDict,
    DecimalContextManager => types::decimal::py_context::ContextManager,
    ForwardRef => types::forward_ref::ForwardRef,
    Exception => exception_private::class_defs::Exception,
    BaseException => exception_private::class_defs::BaseException,
    SystemExit => exception_private::class_defs::SystemExit,
    KeyboardInterrupt => exception_private::class_defs::KeyboardInterrupt,
    ArithmeticError => exception_private::class_defs::ArithmeticError,
    OverflowError => exception_private::class_defs::OverflowError,
    ZeroDivisionError => exception_private::class_defs::ZeroDivisionError,
    LookupError => exception_private::class_defs::LookupError,
    IndexError => exception_private::class_defs::IndexError,
    KeyError => exception_private::class_defs::KeyError,
    RuntimeError => exception_private::class_defs::RuntimeError,
    NotImplementedError => exception_private::class_defs::NotImplementedError,
    RecursionError => exception_private::class_defs::RecursionError,
    AttributeError => exception_private::class_defs::AttributeError,
    FrozenInstanceError => exception_private::class_defs::FrozenInstanceError,
    NameError => exception_private::class_defs::NameError,
    UnboundLocalError => exception_private::class_defs::UnboundLocalError,
    ValueError => exception_private::class_defs::ValueError,
    UnicodeDecodeError => exception_private::class_defs::UnicodeDecodeError,
    UnicodeEncodeError => exception_private::class_defs::UnicodeEncodeError,
    JsonDecodeError => exception_private::class_defs::JsonDecodeError,
    TomlDecodeError => exception_private::class_defs::TomlDecodeError,
    ImportError => exception_private::class_defs::ImportError,
    ModuleNotFoundError => exception_private::class_defs::ModuleNotFoundError,
    OSError => exception_private::class_defs::OSError,
    FileNotFoundError => exception_private::class_defs::FileNotFoundError,
    FileExistsError => exception_private::class_defs::FileExistsError,
    IsADirectoryError => exception_private::class_defs::IsADirectoryError,
    NotADirectoryError => exception_private::class_defs::NotADirectoryError,
    PermissionError => exception_private::class_defs::PermissionError,
    UnsupportedOperation => exception_private::class_defs::UnsupportedOperation,
    TimeoutError => exception_private::class_defs::TimeoutError,
    BlockingIOError => exception_private::class_defs::BlockingIOError,
    AssertionError => exception_private::class_defs::AssertionError,
    MemoryError => exception_private::class_defs::MemoryError,
    StopIteration => exception_private::class_defs::StopIteration,
    SyntaxError => exception_private::class_defs::SyntaxError,
    TypeError => exception_private::class_defs::TypeError,
    RePatternError => exception_private::class_defs::RePatternError,
    BinasciiError => exception_private::class_defs::BinasciiError,
    BinasciiIncomplete => exception_private::class_defs::BinasciiIncomplete,
    CsvError => exception_private::class_defs::CsvError,
    DecimalException => exception_private::class_defs::DecimalException,
    DecimalClamped => exception_private::class_defs::DecimalClamped,
    DecimalInvalidOperation => exception_private::class_defs::DecimalInvalidOperation,
    DecimalConversionSyntax => exception_private::class_defs::DecimalConversionSyntax,
    DecimalDivisionByZero => exception_private::class_defs::DecimalDivisionByZero,
    DecimalDivisionImpossible => exception_private::class_defs::DecimalDivisionImpossible,
    DecimalDivisionUndefined => exception_private::class_defs::DecimalDivisionUndefined,
    DecimalInexact => exception_private::class_defs::DecimalInexact,
    DecimalInvalidContext => exception_private::class_defs::DecimalInvalidContext,
    DecimalRounded => exception_private::class_defs::DecimalRounded,
    DecimalSubnormal => exception_private::class_defs::DecimalSubnormal,
    DecimalOverflow => exception_private::class_defs::DecimalOverflow,
    DecimalUnderflow => exception_private::class_defs::DecimalUnderflow,
    DecimalFloatOperation => exception_private::class_defs::DecimalFloatOperation,
    StatisticsError => exception_private::class_defs::StatisticsError,
    Map => builtins::map::MapClass,
    Filter => builtins::filter::FilterClass,
    Task => asyncio::TaskFuture,
    InvalidStateError => exception_private::class_defs::InvalidStateError,
    ClassMethod => types::builtin_descriptor::ClassMethodClass,
    StaticMethod => types::builtin_descriptor::StaticMethodClass,
}

#[cfg(test)]
mod tests {
    use monty_types::{BuiltinsFunctions, MontyType, ResourceTracker};
    use strum::{IntoEnumIterator, VariantNames};

    use super::{ExcType, Type};
    use crate::{
        builtins::Builtins,
        object_bridge::MontyTypeExt,
        types::{TypeRef, type_ref::c3_merge},
    };

    /// Types hash by name and dicts persist the hash, so every name must stay
    /// the one the pre-table enums gave it; types added since are not listed.
    #[test]
    fn names_match_pre_table_names() {
        let fixture = include_str!("../../tests/fixtures/type_names.txt");
        for line in fixture.lines() {
            let (variant, name) = line.split_once(' ').unwrap();
            let ty = Type::from_variant_name(variant).unwrap_or_else(|| panic!("{variant} was removed"));
            assert_eq!(ty.name(), name, "{variant}");
        }
    }

    /// Native type identity codes and dump names retain the pre-table format independently.
    #[test]
    fn serialization_matches_pre_table_format() {
        let fixture = include_str!("../../tests/fixtures/builtin_type_serialization.txt");
        for line in fixture.lines().filter(|line| !line.starts_with("HostClass ")) {
            let (variant, index) = line.split_once(' ').unwrap();
            let ty = Type::from_variant_name(variant).unwrap_or_else(|| panic!("{variant} was removed"));
            assert_eq!(ty.legacy_index(), index.parse::<u32>().unwrap(), "{variant}");
            let cbor = minicbor_serde::to_vec(ty).unwrap();
            assert_eq!(cbor, minicbor_serde::to_vec(variant).unwrap(), "{variant}");
            assert_eq!(minicbor_serde::from_slice::<Type>(&cbor).unwrap(), ty);
        }
    }

    /// Dumps written before exception classes joined the table stored them as
    /// `Type::Exception(ExcType)` and `Builtins::ExcType`, and `type` as a function.
    #[test]
    fn legacy_shapes_deserialize() {
        #[derive(serde::Serialize)]
        enum LegacyType {
            Exception(ExcType),
        }
        #[derive(serde::Serialize)]
        enum LegacyBuiltins {
            Function(BuiltinsFunctions),
            ExcType(ExcType),
        }
        let cbor = minicbor_serde::to_vec(LegacyType::Exception(ExcType::JsonDecodeError)).unwrap();
        assert_eq!(
            minicbor_serde::from_slice::<Type>(&cbor).unwrap(),
            Type::JsonDecodeError
        );

        let cbor = minicbor_serde::to_vec(LegacyBuiltins::ExcType(ExcType::KeyError)).unwrap();
        let builtin: Builtins = minicbor_serde::from_slice(&cbor).unwrap();
        assert_eq!(builtin, Builtins::Type(Type::KeyError));

        let cbor = minicbor_serde::to_vec(LegacyBuiltins::Function(BuiltinsFunctions::Type)).unwrap();
        let builtin: Builtins = minicbor_serde::from_slice(&cbor).unwrap();
        assert_eq!(builtin, Builtins::Type(Type::Type));
    }

    /// Every written MRO is C3 of its bases, and every walked one only passes
    /// types with a single base, which is when walking the first base is C3.
    #[test]
    fn mros_are_c3_linearisations() {
        for ty in Type::ALL {
            let def = ty.def();
            if def.mro.is_some() {
                let mut sequences: Vec<Vec<TypeRef>> = def
                    .bases
                    .iter()
                    .map(|base| base.mro().map(TypeRef::Native).collect())
                    .collect();
                sequences.push(def.bases.iter().copied().map(TypeRef::Native).collect());
                let mut expected = vec![TypeRef::Native(ty)];
                expected.extend(c3_merge(sequences, &ResourceTracker::default()).unwrap().unwrap());
                let written: Vec<TypeRef> = ty.mro().map(TypeRef::Native).collect();
                assert_eq!(written, expected, "{ty:?}");
            } else {
                for ancestor in ty.mro() {
                    assert!(ancestor.def().bases.len() <= 1, "{ty:?} walks through {ancestor:?}");
                }
            }
            assert_eq!(ty.mro().last(), Some(Type::Object), "{ty:?}");
        }
    }

    /// Every `ExcType` is exactly one exception class and back.
    #[test]
    fn exc_types_round_trip() {
        for name in ExcType::VARIANTS {
            let exc: ExcType = name.parse().unwrap();
            assert_eq!(Type::from(exc).exc_type(), Some(exc));
            assert_eq!(Type::from(exc).name(), *name);
        }
        let exception_classes = Type::ALL.iter().filter(|ty| ty.exc_type().is_some()).count();
        assert_eq!(exception_classes, ExcType::VARIANTS.len());
    }

    /// Every host-side `MontyType` names one native type, under the same name.
    #[test]
    fn monty_types_round_trip() {
        for monty_type in MontyType::iter() {
            let ty = monty_type.to_internal();
            assert_eq!(MontyType::from_internal_static(ty), Some(monty_type), "{monty_type:?}");
            assert_eq!(ty.name(), monty_type.name(), "{monty_type:?}");
        }
    }

    /// The `CallBuiltinType` operand table round-trips.
    #[test]
    fn callable_operands_round_trip() {
        for ty in Type::ALL {
            if let Some(operand) = ty.callable_to_u8() {
                assert_eq!(Type::callable_from_u8(operand), Some(ty));
            }
        }
        assert_eq!(Type::callable_from_u8(17), None);
    }
}
