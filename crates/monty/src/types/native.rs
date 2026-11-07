//! The native type table: one [`TypeDef`] entry per Rust-defined Python type.
//!
//! [`native_types!`] generates the fieldless [`Type`] enum and a static
//! [`TypeDef`] per variant from a single table, so adding a native type is one
//! entry plus its constructor. Facts that used to live in scattered `match`es —
//! names, bases, constructors, class attributes, behaviour flags — live here.
//!
//! Variant names are the dump contract (dumps store them as strings) and the
//! order is the `id()` contract: [`Type::legacy_index`] maps it onto the indices
//! the pre-table enum gave postcard. Append new types at the end of their group.

use std::{fmt, iter::FusedIterator, slice, sync::LazyLock};

use ahash::AHashMap;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
};
use strum::VariantNames;

use crate::{
    args::ArgValues,
    builtins::{iter::builtin_iter, object::object_class_attr, type_::builtin_type},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult, exc_new},
    modules::{collections, csv, hashlib, itertools},
    types::{
        Bytes, Deque, Dict, ForwardRef, FrozenSet, List, Partial, Path, Random, Range, Set, Slice, Str, TimeZone,
        Tuple, bytes, complex, date, datetime, decimal, dict, path, time, timedelta, timezone,
        r#type::{bool_init, float_init, int_init, union_init},
    },
    value::{EitherStr, Value},
};

/// Facts about one native type, read through [`Type::def`].
///
/// Built only by the `const fn` builder methods inside [`native_types!`]; every
/// field defaults to "absent" so an entry names just what differs.
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
    pub ctor: Constructor,
    /// Class-level attributes beyond `__name__` (`date.max`, `timezone.utc`).
    pub class_attr: Option<ClassAttrFn>,
    /// Class-level methods called in place (`dict.fromkeys(...)`).
    pub class_method: Option<ClassMethodFn>,
    /// The boundary exception type, for exception classes.
    pub exc: Option<ExcType>,
}

/// Resolves a class attribute, returning `None` when the type has no such member.
pub(crate) type ClassAttrFn = for<'h> fn(&EitherStr, &mut VM<'h>) -> Option<Value>;

/// Calls a class method in place, handing `args` back as `Err` when the type has
/// no method of that name so the caller can fall back to attribute lookup.
pub(crate) type ClassMethodFn =
    for<'h> fn(&EitherStr, ArgValues, &mut VM<'h>) -> Result<RunResult<CallResult>, ArgValues>;

/// How calling a native type builds its instance.
#[derive(Clone, Copy)]
pub(crate) enum Constructor {
    /// The type cannot be instantiated from Python: calling it raises `TypeError`.
    None,
    /// Builds the value without running Python code.
    Value(for<'h> fn(&mut VM<'h>, ArgValues) -> RunResult<Value>),
    /// May push a frame or suspend (`iter()` calling a user `__iter__`).
    Call(for<'h> fn(&mut VM<'h>, ArgValues) -> RunResult<CallResult>),
    /// Shared by a family of types, so it is told which one was called.
    Typed(for<'h> fn(Type, &mut VM<'h>, ArgValues) -> RunResult<Value>),
}

impl TypeDef {
    /// An entry with `name` and `bases` and everything else absent.
    const fn new(name: &'static str, bases: &'static [Type]) -> Self {
        Self {
            name,
            builtin: None,
            bases,
            mro: None,
            basetype: false,
            class_getitem: false,
            sequence_repeat: false,
            iterator: false,
            ctor: Constructor::None,
            class_attr: None,
            class_method: None,
            exc: None,
        }
    }

    /// The root of the hierarchy: no bases at all.
    const fn root(mut self) -> Self {
        self.bases = &[];
        self
    }

    /// Bound in the builtins namespace under its own name.
    const fn builtin(mut self) -> Self {
        self.builtin = Some(self.name);
        self
    }

    /// Bound in the builtins namespace under a name other than its own.
    const fn builtin_as(mut self, name: &'static str) -> Self {
        self.builtin = Some(name);
        self
    }

    /// Subclassable, as CPython's `Py_TPFLAGS_BASETYPE` marks it.
    const fn basetype(mut self) -> Self {
        self.basetype = true;
        self
    }

    /// Subscripting the type builds a `types.GenericAlias`.
    const fn class_getitem(mut self) -> Self {
        self.class_getitem = true;
        self
    }

    /// Multiplication may fall back to sequence repetition.
    const fn sequence_repeat(mut self) -> Self {
        self.sequence_repeat = true;
        self
    }

    /// One of CPython's concrete iterator types.
    const fn iterator(mut self) -> Self {
        self.iterator = true;
        self
    }

    /// Calling the type builds an instance with `ctor`.
    const fn ctor(mut self, ctor: Constructor) -> Self {
        self.ctor = ctor;
        self
    }

    /// Class-level attributes resolve through `f`.
    const fn class_attr(mut self, f: ClassAttrFn) -> Self {
        self.class_attr = Some(f);
        self
    }

    /// Class methods called in place dispatch through `f`.
    const fn class_method(mut self, f: ClassMethodFn) -> Self {
        self.class_method = Some(f);
        self
    }

    /// The written C3 linearisation, for a type with several bases.
    const fn mro(mut self, mro: &'static [Type]) -> Self {
        self.mro = Some(mro);
        self
    }

    /// An exception class: subclassable, constructed by [`exc_new`], reported to hosts as `exc`.
    const fn exc(mut self, exc: ExcType) -> Self {
        self.exc = Some(exc);
        self.basetype = true;
        self.ctor = Constructor::Typed(exc_new);
        self
    }
}

/// Generates [`Type`], its [`TypeDef`] table and the name lookups from one list.
///
/// Each entry is `Variant("tp_name", Base | Base)` followed by builder calls;
/// omitted bases mean `object`. Doc comments on an entry document the variant.
macro_rules! native_types {
    (@bases) => { &[Type::Object] };
    (@bases $($base:ident)|+) => { &[$(Type::$base),+] };
    ($(
        $(#[doc = $doc:literal])*
        $variant:ident($name:literal $(, $($base:ident)|+)?) $(.$method:ident($($arg:expr),*))*
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
            $(TypeDef::new($name, native_types!(@bases $($($base)|+)?))$(.$method($($arg),*))*,)*
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
        BUILTIN_TYPES.get(name).copied()
    }

    /// The name this type is bound to in the builtins namespace, if any.
    #[must_use]
    pub fn builtin_name(self) -> Option<&'static str> {
        self.def().builtin
    }

    /// The index the pre-table enum gave this type, used for postcard and so `id()`.
    ///
    /// That enum had `HostClass`, `Instance` and `Exception` between `FrozenSet`
    /// and `Function`; the table drops all three, so later types skip their slots.
    pub(crate) fn legacy_index(self) -> u32 {
        const REMOVED_AFTER: u32 = Type::FrozenSet as u32;
        const REMOVED: u32 = 3;
        let index = self as u32;
        if index > REMOVED_AFTER { index + REMOVED } else { index }
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

/// Builtin-namespace names to their types, built once: global loads of `range`,
/// `list` and the like resolve through it on every miss in the module globals.
static BUILTIN_TYPES: LazyLock<AHashMap<&'static str, Type>> = LazyLock::new(|| {
    Type::ALL
        .iter()
        .filter_map(|&ty| ty.builtin_name().map(|name| (name, ty)))
        .collect()
});

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
    Ellipsis("ellipsis"),
    NotImplementedType("NotImplementedType"),
    Type("type")
        .builtin()
        .class_getitem()
        .ctor(Constructor::Value(builtin_type)),
    NoneType("NoneType"),
    Bool("bool", Int)
        .builtin()
        .ctor(Constructor::Value(bool_init)),
    Int("int")
        .builtin()
        .basetype()
        .ctor(Constructor::Value(int_init)),
    Float("float")
        .builtin()
        .basetype()
        .ctor(Constructor::Value(float_init)),
    Complex("complex")
        .builtin()
        .basetype()
        .ctor(Constructor::Value(complex::init))
        .class_method(complex::class_method),
    Range("range")
        .builtin()
        .ctor(Constructor::Value(Range::init)),
    Slice("slice")
        .builtin()
        .ctor(Constructor::Value(Slice::init)),
    /// The four `datetime` classes are qualified like `collections.deque`:
    /// this is the `tp_name` CPython gives these C types, so it is the
    /// spelling its reprs and type-naming error messages use. `__name__`
    /// reports the bare name, see `dunder_name`.
    Date("datetime.date")
        .basetype()
        .ctor(Constructor::Value(date::init))
        .class_attr(date::class_attr)
        .class_method(date::class_method),
    DateTime("datetime.datetime", Date)
        .basetype()
        .ctor(Constructor::Value(datetime::init))
        .class_attr(datetime::class_attr)
        .class_method(datetime::class_method),
    TimeDelta("datetime.timedelta")
        .basetype()
        .ctor(Constructor::Value(timedelta::init))
        .class_attr(timedelta::class_attr),
    TimeZone("datetime.timezone")
        .ctor(Constructor::Value(TimeZone::init))
        .class_attr(timezone::class_attr),
    Str("str")
        .builtin()
        .basetype()
        .sequence_repeat()
        .ctor(Constructor::Value(Str::init)),
    Bytes("bytes")
        .builtin()
        .basetype()
        .sequence_repeat()
        .ctor(Constructor::Value(Bytes::init))
        .class_method(bytes::bytes_class_method),
    List("list")
        .builtin()
        .basetype()
        .class_getitem()
        .sequence_repeat()
        .ctor(Constructor::Value(List::init)),
    Tuple("tuple")
        .builtin()
        .basetype()
        .class_getitem()
        .sequence_repeat()
        .ctor(Constructor::Value(Tuple::init)),
    NamedTuple("namedtuple", Tuple)
        .sequence_repeat(),
    Dict("dict")
        .builtin()
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(Dict::init))
        .class_method(dict::dict_class_method),
    /// `collections.defaultdict` — a `dict` with a `default_factory` (stored as
    /// a `DictKind` on the `Dict`, not a separate heap type).
    DefaultDict("collections.defaultdict", Dict)
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(collections::defaultdict_init))
        .class_method(collections::defaultdict_class_method),
    /// `collections.Counter` — a `dict` subclass (also a `DictKind`).
    Counter("Counter", Dict)
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(collections::counter_init))
        .class_method(collections::counter_class_method),
    DictKeys("dict_keys"),
    DictItems("dict_items"),
    DictValues("dict_values"),
    Set("set")
        .builtin()
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(Set::init)),
    FrozenSet("frozenset")
        .builtin()
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(FrozenSet::init)),
    Function("function"),
    BuiltinFunction("builtin_function_or_method"),
    Cell("cell"),
    Iterator("iterator")
        .builtin_as("iter")
        .ctor(Constructor::Call(builtin_iter)),
    ListIterator("list_iterator")
        .iterator(),
    CallableIterator("callable_iterator")
        .iterator(),
    /// Coroutine type for async functions and external futures.
    Coroutine("coroutine"),
    Module("module"),
    /// Marker types like stdout/stderr - displays as "_io.TextIOWrapper"
    TextIOWrapper("_io.TextIOWrapper"),
    /// Binary file object returned by `open(..., "rb")`.
    BufferedReader("_io.BufferedReader"),
    /// Binary file object returned by write-only binary modes.
    BufferedWriter("_io.BufferedWriter"),
    /// Binary file object returned by read/write binary modes.
    BufferedRandom("_io.BufferedRandom"),
    /// typing module special forms (Any, Optional, Union, etc.) - displays as "typing._SpecialForm"
    SpecialForm("typing._SpecialForm"),
    /// A filesystem path from `pathlib.Path` - displays as "PosixPath"
    Path("PosixPath")
        .basetype()
        .ctor(Constructor::Value(Path::init))
        .class_method(path::class_method),
    /// A property descriptor - displays as "property"
    Property("property")
        .builtin()
        .basetype(),
    /// A compiled regex pattern from `re.compile()` - displays as "re.Pattern"
    RePattern("re.Pattern")
        .class_getitem(),
    /// A regex match result from `re.match()` / `re.search()` etc. - displays as "re.Match"
    ReMatch("re.Match")
        .class_getitem(),
    /// An `os.DirEntry` from `os.scandir()`.
    DirEntry("posix.DirEntry"),
    /// The iterator `os.scandir()` returns.
    ScandirIterator("posix.ScandirIterator")
        .iterator(),
    /// The iterator `os.walk()` and `Path.walk()` return; CPython's is a generator.
    WalkGenerator("generator")
        .iterator(),
    TupleIterator("tuple_iterator")
        .iterator(),
    StrAsciiIterator("str_ascii_iterator")
        .iterator(),
    StrIterator("str_iterator")
        .iterator(),
    BytesIterator("bytes_iterator")
        .iterator(),
    RangeIterator("range_iterator")
        .iterator(),
    DictKeyIterator("dict_keyiterator")
        .iterator(),
    DictItemIterator("dict_itemiterator")
        .iterator(),
    DictValueIterator("dict_valueiterator")
        .iterator(),
    SetIterator("set_iterator")
        .iterator(),
    ItertoolsCount("itertools.count")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsRepeat("itertools.repeat")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// A `dataclasses.Field` from a class's `__dataclass_fields__` — displays
    /// as "Field", the name CPython's `Field.__name__` reports.
    DataclassField("Field"),
    /// `collections.deque` — qualified like `datetime.datetime`/`re.Pattern` so
    /// the name matches CPython's `repr` and error messages; `__name__` reports
    /// the bare `'deque'`, see `dunder_name`.
    Deque("collections.deque")
        .basetype()
        .class_getitem()
        .sequence_repeat()
        .ctor(Constructor::Value(Deque::init)),
    /// `iter(deque(...))` — CPython's `_collections._deque_iterator`.
    DequeIterator("_collections._deque_iterator")
        .iterator(),
    ItertoolsPairwise("itertools.pairwise")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsCompress("itertools.compress")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsIslice("itertools.islice")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsChain("itertools.chain")
        .class_getitem()
        .iterator()
        .ctor(Constructor::Typed(itertools::construct))
        .class_attr(itertools::chain_class_attr),
    ItertoolsCycle("itertools.cycle")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// The `__dataclass_params__` of a `@dataclass`, named as CPython's
    /// private `dataclasses._DataclassParams` reports itself.
    DataclassParams("_DataclassParams"),
    /// The type of `dataclasses.MISSING`, CPython's private `_MISSING_TYPE`.
    MissingType("dataclasses._MISSING_TYPE"),
    ItertoolsTakeWhile("itertools.takewhile")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsDropWhile("itertools.dropwhile")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsFilterFalse("itertools.filterfalse")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsStarMap("itertools.starmap")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// `object`, the root of every class. Its members (`__init__`, `__new__`,
    /// `__init_subclass__`, `__setattr__`) are inherited by every class, but
    /// `object()` itself cannot be built (see `limitations/classes.md`).
    Object("object")
        .root()
        .builtin()
        .basetype()
        .class_attr(object_class_attr),
    Time("datetime.time")
        .basetype()
        .ctor(Constructor::Value(time::init))
        .class_attr(time::class_attr)
        .class_method(time::class_method),
    /// `functools.partial` — qualified like `collections.deque`, so
    /// `type(p)` reads `<class 'functools.partial'>`.
    Partial("functools.partial")
        .basetype()
        .class_getitem()
        .ctor(Constructor::Value(Partial::init)),
    ItertoolsAccumulate("itertools.accumulate")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsBatched("itertools.batched")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsZipLongest("itertools.zip_longest")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// `types.GenericAlias`, the type of `list[int]`; qualified like
    /// `functools.partial`, so `type(list[int])` reads `<class 'types.GenericAlias'>`.
    GenericAlias("types.GenericAlias"),
    /// `typing.Union`, the type of `int | None` — one object with
    /// `types.UnionType` since CPython 3.14, and the value bound to
    /// `typing.Union` itself.
    Union("typing.Union")
        .ctor(Constructor::Value(union_init)),
    ItertoolsCombinations("itertools.combinations")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsCombinationsWithReplacement("itertools.combinations_with_replacement")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsPermutations("itertools.permutations")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsProduct("itertools.product")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    ItertoolsGroupBy("itertools.groupby")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// The sub-iterator `groupby` yields for each group — CPython's private
    /// `itertools._grouper`, named as such so `type()` matches.
    ItertoolsGrouper("itertools._grouper")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// The iterator `tee()` hands out — CPython's private `itertools._tee`,
    /// which it nonetheless exposes on the module.
    ItertoolsTee("itertools._tee")
        .iterator()
        .ctor(Constructor::Typed(itertools::construct)),
    /// The read-ahead buffer those iterators share, CPython's
    /// `itertools._tee_dataobject`. Never reachable from Python except as
    /// that name.
    ItertoolsTeeDataObject("itertools._tee_dataobject")
        .ctor(Constructor::Typed(itertools::construct)),
    /// `random.Random`, qualified like `collections.deque` so `type(rng)`
    /// reads `<class 'random.Random'>`.
    Random("random.Random")
        .basetype()
        .ctor(Constructor::Value(Random::init)),
    /// The wrapper returned by caching decorators. Monty exposes this type
    /// through `type(f)`, but not as a `functools` module attribute.
    LruCacheWrapper("functools._lru_cache_wrapper"),
    /// The OpenSSL-backed `hashlib` hash object, `_hashlib.HASH`.
    HashlibHash("_hashlib.HASH"),
    /// The `shake_128` / `shake_256` hash object, `_hashlib.HASHXOF`.
    HashlibHashXof("_hashlib.HASHXOF"),
    /// `hashlib.blake2b`, a type in CPython and so callable here.
    Blake2b("_blake2.blake2b")
        .ctor(Constructor::Value(hashlib::blake2b_init))
        .class_attr(hashlib::blake2b_class_attr),
    /// `hashlib.blake2s`, a type in CPython and so callable here.
    Blake2s("_blake2.blake2s")
        .ctor(Constructor::Value(hashlib::blake2s_init))
        .class_attr(hashlib::blake2s_class_attr),
    /// The iterator `csv.reader()` returns, CPython's `_csv.reader`.
    CsvReader("_csv.reader")
        .iterator(),
    /// The object `csv.writer()` returns, CPython's `_csv.writer`.
    CsvWriter("_csv.writer"),
    /// `csv.DictReader`, a class in CPython and so callable here.
    CsvDictReader("csv.DictReader")
        .iterator()
        .ctor(Constructor::Call(csv::dict_reader_call)),
    /// `csv.DictWriter`, a class in CPython and so callable here.
    CsvDictWriter("csv.DictWriter")
        .ctor(Constructor::Value(csv::dict_writer_init)),
    /// A resolved `csv` dialect, CPython's `_csv.Dialect`.
    CsvDialect("_csv.Dialect")
        .ctor(Constructor::Value(csv::dialect_init)),
    /// `decimal.Decimal`.
    Decimal("decimal.Decimal")
        .basetype()
        .ctor(Constructor::Value(decimal::py::init))
        .class_method(decimal::py::class_method),
    /// `decimal.Context`.
    DecimalContext("decimal.Context")
        .ctor(Constructor::Value(decimal::py_context::init)),
    /// The type of a context's `flags` / `traps`; CPython defines it in Python, so
    /// its name carries no module.
    DecimalSignalDict("SignalDict"),
    /// The manager `decimal.localcontext()` returns.
    DecimalContextManager("decimal.ContextManager"),
    /// `annotationlib.ForwardRef`, an unevaluated annotation.
    ForwardRef("annotationlib.ForwardRef")
        .ctor(Constructor::Value(ForwardRef::init)),
    /// The base of every ordinary exception.
    Exception("Exception", BaseException)
        .builtin()
        .exc(ExcType::Exception),
    /// The root of every exception class.
    BaseException("BaseException", Object)
        .builtin()
        .exc(ExcType::BaseException),
    SystemExit("SystemExit", BaseException)
        .builtin()
        .exc(ExcType::SystemExit),
    KeyboardInterrupt("KeyboardInterrupt", BaseException)
        .builtin()
        .exc(ExcType::KeyboardInterrupt),
    ArithmeticError("ArithmeticError", Exception)
        .builtin()
        .exc(ExcType::ArithmeticError),
    OverflowError("OverflowError", ArithmeticError)
        .builtin()
        .exc(ExcType::OverflowError),
    ZeroDivisionError("ZeroDivisionError", ArithmeticError)
        .builtin()
        .exc(ExcType::ZeroDivisionError),
    LookupError("LookupError", Exception)
        .builtin()
        .exc(ExcType::LookupError),
    IndexError("IndexError", LookupError)
        .builtin()
        .exc(ExcType::IndexError),
    KeyError("KeyError", LookupError)
        .builtin()
        .exc(ExcType::KeyError),
    RuntimeError("RuntimeError", Exception)
        .builtin()
        .exc(ExcType::RuntimeError),
    NotImplementedError("NotImplementedError", RuntimeError)
        .builtin()
        .exc(ExcType::NotImplementedError),
    RecursionError("RecursionError", RuntimeError)
        .builtin()
        .exc(ExcType::RecursionError),
    AttributeError("AttributeError", Exception)
        .builtin()
        .exc(ExcType::AttributeError),
    /// Subclass of AttributeError (from dataclasses module).
    FrozenInstanceError("FrozenInstanceError", AttributeError)
        .exc(ExcType::FrozenInstanceError),
    NameError("NameError", Exception)
        .builtin()
        .exc(ExcType::NameError),
    /// Subclass of NameError - for accessing local variable before assignment.
    UnboundLocalError("UnboundLocalError", NameError)
        .builtin()
        .exc(ExcType::UnboundLocalError),
    ValueError("ValueError", Exception)
        .builtin()
        .exc(ExcType::ValueError),
    /// Subclass of ValueError - for encoding/decoding errors.
    UnicodeDecodeError("UnicodeDecodeError", ValueError)
        .builtin()
        .exc(ExcType::UnicodeDecodeError),
    /// Subclass of ValueError - for encoding errors (e.g. `str.encode('ascii')`
    /// on a string containing non-ASCII characters).
    UnicodeEncodeError("UnicodeEncodeError", ValueError)
        .builtin()
        .exc(ExcType::UnicodeEncodeError),
    /// Subclass of ValueError for invalid JSON syntax in `json.loads()`.
    JsonDecodeError("json.JSONDecodeError", ValueError)
        .exc(ExcType::JsonDecodeError),
    /// Subclass of ValueError for invalid TOML in `tomllib.loads()` / `tomllib.load()`.
    TomlDecodeError("tomllib.TOMLDecodeError", ValueError)
        .exc(ExcType::TomlDecodeError),
    /// Import-related errors (module not found, name not in module).
    ImportError("ImportError", Exception)
        .builtin()
        .exc(ExcType::ImportError),
    /// Subclass of ImportError - for when a module cannot be found.
    ModuleNotFoundError("ModuleNotFoundError", ImportError)
        .builtin()
        .exc(ExcType::ModuleNotFoundError),
    /// OS-related errors (file not found, permission denied, etc.)
    OSError("OSError", Exception)
        .builtin()
        .exc(ExcType::OSError),
    /// Subclass of OSError - for when a file or directory cannot be found.
    FileNotFoundError("FileNotFoundError", OSError)
        .builtin()
        .exc(ExcType::FileNotFoundError),
    /// Subclass of OSError - for when a file already exists.
    FileExistsError("FileExistsError", OSError)
        .builtin()
        .exc(ExcType::FileExistsError),
    /// Subclass of OSError - for when a path is a directory but a file was expected.
    IsADirectoryError("IsADirectoryError", OSError)
        .builtin()
        .exc(ExcType::IsADirectoryError),
    /// Subclass of OSError - for when a path is not a directory but one was expected.
    NotADirectoryError("NotADirectoryError", OSError)
        .builtin()
        .exc(ExcType::NotADirectoryError),
    /// Subclass of OSError - for when an operation is not permitted (e.g., writing
    /// to a read-only mount, or attempting to access a path outside a mounted directory).
    PermissionError("PermissionError", OSError)
        .builtin()
        .exc(ExcType::PermissionError),
    /// `io.UnsupportedOperation` - raised by file objects when a requested
    /// operation isn't allowed by the open mode (e.g. `read()` on `'w'`).
    ///
    /// It derives from both `OSError` and `ValueError`, as in CPython, so
    /// `except ValueError:` and `except OSError:` both catch it.
    UnsupportedOperation("io.UnsupportedOperation", OSError | ValueError)
        .exc(ExcType::UnsupportedOperation)
        .mro(&[
            Type::UnsupportedOperation, Type::OSError, Type::ValueError, Type::Exception,
            Type::BaseException, Type::Object,
        ]),
    /// Subclass of OSError since Python 3.3 (PEP 3151).
    TimeoutError("TimeoutError", OSError)
        .builtin()
        .exc(ExcType::TimeoutError),
    AssertionError("AssertionError", Exception)
        .builtin()
        .exc(ExcType::AssertionError),
    MemoryError("MemoryError", Exception)
        .builtin()
        .exc(ExcType::MemoryError),
    StopIteration("StopIteration", Exception)
        .builtin()
        .exc(ExcType::StopIteration),
    SyntaxError("SyntaxError", Exception)
        .builtin()
        .exc(ExcType::SyntaxError),
    TypeError("TypeError", Exception)
        .builtin()
        .exc(ExcType::TypeError),
    /// `re.PatternError` - raised for invalid regex patterns or unsupported regex features.
    ///
    /// # Behavior Note
    ///
    /// Limited to monty's exception type, `PatternError` does not provide `pattern`, `pos`,
    /// `lineno` and `colno` attributes.
    ///
    /// As per CPython's implementation, it would be hard to convert `fancy-regex`'s error
    /// representations into the required attributes.
    RePatternError("re.PatternError", Exception)
        .exc(ExcType::RePatternError),
    /// `binascii.Error` - raised by the `base64` codecs for malformed input.
    ///
    /// A `ValueError` subclass in CPython, so `except ValueError:` catches it.
    BinasciiError("binascii.Error", ValueError)
        .exc(ExcType::BinasciiError),
    /// `binascii.Incomplete` - a direct `Exception` subclass, not a `ValueError`.
    ///
    /// Nothing raises it: the `a2b_hqx` family it belonged to left CPython in
    /// 3.11, so the class survives only for `except binascii.Incomplete:` in
    /// older code. Monty exposes it for the same reason.
    BinasciiIncomplete("binascii.Incomplete", Exception)
        .exc(ExcType::BinasciiIncomplete),
    /// `csv.Error` - raised by the `csv` readers and writers for malformed data
    /// and unknown dialect names.
    ///
    /// A direct `Exception` subclass, named `_csv.Error` as in CPython.
    CsvError("_csv.Error", Exception)
        .exc(ExcType::CsvError),
    /// `decimal.DecimalException`, the base of every decimal signal.
    DecimalException("decimal.DecimalException", ArithmeticError)
        .exc(ExcType::DecimalException),
    /// `decimal.Clamped`: an exponent was changed to fit the representation.
    DecimalClamped("decimal.Clamped", DecimalException)
        .exc(ExcType::DecimalClamped),
    /// `decimal.InvalidOperation`: an operation with no meaningful result.
    DecimalInvalidOperation("decimal.InvalidOperation", DecimalException)
        .exc(ExcType::DecimalInvalidOperation),
    /// `decimal.ConversionSyntax`: a malformed numeric string.
    DecimalConversionSyntax("decimal.ConversionSyntax", DecimalInvalidOperation)
        .exc(ExcType::DecimalConversionSyntax),
    /// `decimal.DivisionByZero`: a finite non-zero dividend divided by zero.
    DecimalDivisionByZero("decimal.DivisionByZero", DecimalException | ZeroDivisionError)
        .exc(ExcType::DecimalDivisionByZero)
        .mro(&[
            Type::DecimalDivisionByZero, Type::DecimalException, Type::ZeroDivisionError,
            Type::ArithmeticError, Type::Exception, Type::BaseException, Type::Object,
        ]),
    /// `decimal.DivisionImpossible`: an integer quotient too long for the precision.
    DecimalDivisionImpossible("decimal.DivisionImpossible", DecimalInvalidOperation)
        .exc(ExcType::DecimalDivisionImpossible),
    /// `decimal.DivisionUndefined`: zero divided by zero.
    DecimalDivisionUndefined("decimal.DivisionUndefined", DecimalInvalidOperation | ZeroDivisionError)
        .exc(ExcType::DecimalDivisionUndefined)
        .mro(&[
            Type::DecimalDivisionUndefined, Type::DecimalInvalidOperation, Type::DecimalException,
            Type::ZeroDivisionError, Type::ArithmeticError, Type::Exception, Type::BaseException,
            Type::Object,
        ]),
    /// `decimal.Inexact`: a result was rounded and lost non-zero digits.
    DecimalInexact("decimal.Inexact", DecimalException)
        .exc(ExcType::DecimalInexact),
    /// `decimal.InvalidContext`: an invalid context setting was detected.
    DecimalInvalidContext("decimal.InvalidContext", DecimalInvalidOperation)
        .exc(ExcType::DecimalInvalidContext),
    /// `decimal.Rounded`: a result was rounded, perhaps without losing digits.
    DecimalRounded("decimal.Rounded", DecimalException)
        .exc(ExcType::DecimalRounded),
    /// `decimal.Subnormal`: a result's exponent fell below `Emin`.
    DecimalSubnormal("decimal.Subnormal", DecimalException)
        .exc(ExcType::DecimalSubnormal),
    /// `decimal.Overflow`: a result's exponent exceeded `Emax`.
    DecimalOverflow("decimal.Overflow", DecimalInexact | DecimalRounded)
        .exc(ExcType::DecimalOverflow)
        .mro(&[
            Type::DecimalOverflow, Type::DecimalInexact, Type::DecimalRounded, Type::DecimalException,
            Type::ArithmeticError, Type::Exception, Type::BaseException, Type::Object,
        ]),
    /// `decimal.Underflow`: a subnormal result was rounded.
    DecimalUnderflow("decimal.Underflow", DecimalInexact | DecimalRounded | DecimalSubnormal)
        .exc(ExcType::DecimalUnderflow)
        .mro(&[
            Type::DecimalUnderflow, Type::DecimalInexact, Type::DecimalRounded, Type::DecimalSubnormal,
            Type::DecimalException, Type::ArithmeticError, Type::Exception, Type::BaseException,
            Type::Object,
        ]),
    /// `decimal.FloatOperation`: floats and Decimals were mixed.
    DecimalFloatOperation("decimal.FloatOperation", DecimalException | TypeError)
        .exc(ExcType::DecimalFloatOperation)
        .mro(&[
            Type::DecimalFloatOperation, Type::DecimalException, Type::ArithmeticError,
            Type::TypeError, Type::Exception, Type::BaseException, Type::Object,
        ]),
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

    /// `id()` encodes types through postcard, so the legacy indices the
    /// pre-table enum gave postcard must hold; dumps store the variant names.
    #[test]
    fn serialization_matches_pre_table_format() {
        let fixture = include_str!("../../tests/fixtures/builtin_type_serialization.txt");
        for line in fixture.lines().filter(|line| !line.starts_with("HostClass ")) {
            let (variant, index) = line.split_once(' ').unwrap();
            let ty = Type::from_variant_name(variant).unwrap_or_else(|| panic!("{variant} was removed"));
            assert_eq!(postcard::to_allocvec(&ty).unwrap(), vec![index.parse::<u8>().unwrap()]);
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
        assert_eq!(Type::callable_from_u8(15), None);
    }
}
