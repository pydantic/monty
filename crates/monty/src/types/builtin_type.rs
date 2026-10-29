use std::fmt;

use crate::{exception_private::ExcType, types::Type};

/// Type objects stored inline in values; user classes use heap references.
/// Serialize through `Type` to retain its dump names and postcard indices,
/// including the gap left by `Type::Instance`. Keep both conversions exhaustive.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::VariantNames,
)]
#[strum(serialize_all = "lowercase")]
#[serde(into = "Type", try_from = "Type")]
pub(crate) enum BuiltinType {
    Ellipsis,
    #[strum(serialize = "NotImplementedType")]
    NotImplementedType,
    Type,
    #[strum(serialize = "NoneType")]
    NoneType,
    Bool,
    Int,
    Float,
    Complex,
    Range,
    Slice,
    #[strum(serialize = "datetime.date")]
    Date,
    #[strum(serialize = "datetime.datetime")]
    DateTime,
    #[strum(serialize = "datetime.timedelta")]
    TimeDelta,
    #[strum(serialize = "datetime.timezone")]
    TimeZone,
    Str,
    Bytes,
    List,
    Tuple,
    NamedTuple,
    Dict,
    #[strum(serialize = "collections.defaultdict")]
    DefaultDict,
    #[strum(serialize = "Counter")]
    Counter,
    #[strum(serialize = "dict_keys")]
    DictKeys,
    #[strum(serialize = "dict_items")]
    DictItems,
    #[strum(serialize = "dict_values")]
    DictValues,
    Set,
    FrozenSet,
    #[strum(serialize = "HostClass")]
    HostClass,
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    #[strum(serialize = "builtin_function_or_method")]
    BuiltinFunction,
    Cell,
    Iterator,
    #[strum(serialize = "list_iterator")]
    ListIterator,
    #[strum(serialize = "callable_iterator")]
    CallableIterator,
    Coroutine,
    Module,
    #[strum(serialize = "_io.TextIOWrapper")]
    TextIOWrapper,
    #[strum(serialize = "_io.BufferedReader")]
    BufferedReader,
    #[strum(serialize = "_io.BufferedWriter")]
    BufferedWriter,
    #[strum(serialize = "_io.BufferedRandom")]
    BufferedRandom,
    #[strum(serialize = "typing._SpecialForm")]
    SpecialForm,
    #[strum(serialize = "PosixPath")]
    Path,
    Property,
    #[strum(serialize = "re.Pattern")]
    RePattern,
    #[strum(serialize = "re.Match")]
    ReMatch,
    #[strum(serialize = "posix.DirEntry")]
    DirEntry,
    #[strum(serialize = "posix.ScandirIterator")]
    ScandirIterator,
    #[strum(serialize = "generator")]
    WalkGenerator,
    #[strum(serialize = "tuple_iterator")]
    TupleIterator,
    #[strum(serialize = "str_ascii_iterator")]
    StrAsciiIterator,
    #[strum(serialize = "str_iterator")]
    StrIterator,
    #[strum(serialize = "bytes_iterator")]
    BytesIterator,
    #[strum(serialize = "range_iterator")]
    RangeIterator,
    #[strum(serialize = "dict_keyiterator")]
    DictKeyIterator,
    #[strum(serialize = "dict_itemiterator")]
    DictItemIterator,
    #[strum(serialize = "dict_valueiterator")]
    DictValueIterator,
    #[strum(serialize = "set_iterator")]
    SetIterator,
    #[strum(serialize = "itertools.count")]
    ItertoolsCount,
    #[strum(serialize = "itertools.repeat")]
    ItertoolsRepeat,
    #[strum(serialize = "Field")]
    DataclassField,
    #[strum(serialize = "collections.deque")]
    Deque,
    #[strum(serialize = "_collections._deque_iterator")]
    DequeIterator,
    #[strum(serialize = "itertools.pairwise")]
    ItertoolsPairwise,
    #[strum(serialize = "itertools.compress")]
    ItertoolsCompress,
    #[strum(serialize = "itertools.islice")]
    ItertoolsIslice,
    #[strum(serialize = "itertools.chain")]
    ItertoolsChain,
    #[strum(serialize = "itertools.cycle")]
    ItertoolsCycle,
    #[strum(serialize = "_DataclassParams")]
    DataclassParams,
    #[strum(serialize = "dataclasses._MISSING_TYPE")]
    MissingType,
    #[strum(serialize = "itertools.takewhile")]
    ItertoolsTakeWhile,
    #[strum(serialize = "itertools.dropwhile")]
    ItertoolsDropWhile,
    #[strum(serialize = "itertools.filterfalse")]
    ItertoolsFilterFalse,
    #[strum(serialize = "itertools.starmap")]
    ItertoolsStarMap,
    Object,
    #[strum(serialize = "datetime.time")]
    Time,
    #[strum(serialize = "functools.partial")]
    Partial,
    #[strum(serialize = "itertools.accumulate")]
    ItertoolsAccumulate,
    #[strum(serialize = "itertools.batched")]
    ItertoolsBatched,
    #[strum(serialize = "itertools.zip_longest")]
    ItertoolsZipLongest,
    #[strum(serialize = "types.GenericAlias")]
    GenericAlias,
    #[strum(serialize = "typing.Union")]
    Union,
    #[strum(serialize = "itertools.combinations")]
    ItertoolsCombinations,
    #[strum(serialize = "itertools.combinations_with_replacement")]
    ItertoolsCombinationsWithReplacement,
    #[strum(serialize = "itertools.permutations")]
    ItertoolsPermutations,
    #[strum(serialize = "itertools.product")]
    ItertoolsProduct,
    #[strum(serialize = "itertools.groupby")]
    ItertoolsGroupBy,
    #[strum(serialize = "itertools._grouper")]
    ItertoolsGrouper,
    #[strum(serialize = "itertools._tee")]
    ItertoolsTee,
    #[strum(serialize = "itertools._tee_dataobject")]
    ItertoolsTeeDataObject,
    #[strum(serialize = "random.Random")]
    Random,
    #[strum(serialize = "functools._lru_cache_wrapper")]
    LruCacheWrapper,
    #[strum(serialize = "_hashlib.HASH")]
    HashlibHash,
    #[strum(serialize = "_hashlib.HASHXOF")]
    HashlibHashXof,
    #[strum(serialize = "_blake2.blake2b")]
    Blake2b,
    #[strum(serialize = "_blake2.blake2s")]
    Blake2s,
    #[strum(serialize = "_csv.reader")]
    CsvReader,
    #[strum(serialize = "_csv.writer")]
    CsvWriter,
    #[strum(serialize = "csv.DictReader")]
    CsvDictReader,
    #[strum(serialize = "csv.DictWriter")]
    CsvDictWriter,
    #[strum(serialize = "_csv.Dialect")]
    CsvDialect,
    #[strum(serialize = "decimal.Decimal")]
    Decimal,
    #[strum(serialize = "decimal.Context")]
    DecimalContext,
    #[strum(serialize = "SignalDict")]
    DecimalSignalDict,
    #[strum(serialize = "decimal.ContextManager")]
    DecimalContextManager,
    #[strum(serialize = "ipaddress.IPv4Address")]
    IPv4Address,
    #[strum(serialize = "ipaddress.IPv6Address")]
    IPv6Address,
    #[strum(serialize = "ipaddress.IPv4Network")]
    IPv4Network,
    #[strum(serialize = "ipaddress.IPv6Network")]
    IPv6Network,
}

impl From<BuiltinType> for Type {
    fn from(value: BuiltinType) -> Self {
        match value {
            BuiltinType::Ellipsis => Self::Ellipsis,
            BuiltinType::NotImplementedType => Self::NotImplementedType,
            BuiltinType::Type => Self::Type,
            BuiltinType::NoneType => Self::NoneType,
            BuiltinType::Bool => Self::Bool,
            BuiltinType::Int => Self::Int,
            BuiltinType::Float => Self::Float,
            BuiltinType::Complex => Self::Complex,
            BuiltinType::Range => Self::Range,
            BuiltinType::Slice => Self::Slice,
            BuiltinType::Date => Self::Date,
            BuiltinType::DateTime => Self::DateTime,
            BuiltinType::TimeDelta => Self::TimeDelta,
            BuiltinType::TimeZone => Self::TimeZone,
            BuiltinType::Str => Self::Str,
            BuiltinType::Bytes => Self::Bytes,
            BuiltinType::List => Self::List,
            BuiltinType::Tuple => Self::Tuple,
            BuiltinType::NamedTuple => Self::NamedTuple,
            BuiltinType::Dict => Self::Dict,
            BuiltinType::DefaultDict => Self::DefaultDict,
            BuiltinType::Counter => Self::Counter,
            BuiltinType::DictKeys => Self::DictKeys,
            BuiltinType::DictItems => Self::DictItems,
            BuiltinType::DictValues => Self::DictValues,
            BuiltinType::Set => Self::Set,
            BuiltinType::FrozenSet => Self::FrozenSet,
            BuiltinType::HostClass => Self::HostClass,
            BuiltinType::Exception(exc_type) => Self::Exception(exc_type),
            BuiltinType::Function => Self::Function,
            BuiltinType::BuiltinFunction => Self::BuiltinFunction,
            BuiltinType::Cell => Self::Cell,
            BuiltinType::Iterator => Self::Iterator,
            BuiltinType::ListIterator => Self::ListIterator,
            BuiltinType::CallableIterator => Self::CallableIterator,
            BuiltinType::Coroutine => Self::Coroutine,
            BuiltinType::Module => Self::Module,
            BuiltinType::TextIOWrapper => Self::TextIOWrapper,
            BuiltinType::BufferedReader => Self::BufferedReader,
            BuiltinType::BufferedWriter => Self::BufferedWriter,
            BuiltinType::BufferedRandom => Self::BufferedRandom,
            BuiltinType::SpecialForm => Self::SpecialForm,
            BuiltinType::Path => Self::Path,
            BuiltinType::Property => Self::Property,
            BuiltinType::RePattern => Self::RePattern,
            BuiltinType::ReMatch => Self::ReMatch,
            BuiltinType::DirEntry => Self::DirEntry,
            BuiltinType::ScandirIterator => Self::ScandirIterator,
            BuiltinType::WalkGenerator => Self::WalkGenerator,
            BuiltinType::TupleIterator => Self::TupleIterator,
            BuiltinType::StrAsciiIterator => Self::StrAsciiIterator,
            BuiltinType::StrIterator => Self::StrIterator,
            BuiltinType::BytesIterator => Self::BytesIterator,
            BuiltinType::RangeIterator => Self::RangeIterator,
            BuiltinType::DictKeyIterator => Self::DictKeyIterator,
            BuiltinType::DictItemIterator => Self::DictItemIterator,
            BuiltinType::DictValueIterator => Self::DictValueIterator,
            BuiltinType::SetIterator => Self::SetIterator,
            BuiltinType::ItertoolsCount => Self::ItertoolsCount,
            BuiltinType::ItertoolsRepeat => Self::ItertoolsRepeat,
            BuiltinType::DataclassField => Self::DataclassField,
            BuiltinType::Deque => Self::Deque,
            BuiltinType::DequeIterator => Self::DequeIterator,
            BuiltinType::ItertoolsPairwise => Self::ItertoolsPairwise,
            BuiltinType::ItertoolsCompress => Self::ItertoolsCompress,
            BuiltinType::ItertoolsIslice => Self::ItertoolsIslice,
            BuiltinType::ItertoolsChain => Self::ItertoolsChain,
            BuiltinType::ItertoolsCycle => Self::ItertoolsCycle,
            BuiltinType::DataclassParams => Self::DataclassParams,
            BuiltinType::MissingType => Self::MissingType,
            BuiltinType::ItertoolsTakeWhile => Self::ItertoolsTakeWhile,
            BuiltinType::ItertoolsDropWhile => Self::ItertoolsDropWhile,
            BuiltinType::ItertoolsFilterFalse => Self::ItertoolsFilterFalse,
            BuiltinType::ItertoolsStarMap => Self::ItertoolsStarMap,
            BuiltinType::Object => Self::Object,
            BuiltinType::Time => Self::Time,
            BuiltinType::Partial => Self::Partial,
            BuiltinType::ItertoolsAccumulate => Self::ItertoolsAccumulate,
            BuiltinType::ItertoolsBatched => Self::ItertoolsBatched,
            BuiltinType::ItertoolsZipLongest => Self::ItertoolsZipLongest,
            BuiltinType::GenericAlias => Self::GenericAlias,
            BuiltinType::Union => Self::Union,
            BuiltinType::ItertoolsCombinations => Self::ItertoolsCombinations,
            BuiltinType::ItertoolsCombinationsWithReplacement => Self::ItertoolsCombinationsWithReplacement,
            BuiltinType::ItertoolsPermutations => Self::ItertoolsPermutations,
            BuiltinType::ItertoolsProduct => Self::ItertoolsProduct,
            BuiltinType::ItertoolsGroupBy => Self::ItertoolsGroupBy,
            BuiltinType::ItertoolsGrouper => Self::ItertoolsGrouper,
            BuiltinType::ItertoolsTee => Self::ItertoolsTee,
            BuiltinType::ItertoolsTeeDataObject => Self::ItertoolsTeeDataObject,
            BuiltinType::Random => Self::Random,
            BuiltinType::LruCacheWrapper => Self::LruCacheWrapper,
            BuiltinType::HashlibHash => Self::HashlibHash,
            BuiltinType::HashlibHashXof => Self::HashlibHashXof,
            BuiltinType::Blake2b => Self::Blake2b,
            BuiltinType::Blake2s => Self::Blake2s,
            BuiltinType::CsvReader => Self::CsvReader,
            BuiltinType::CsvWriter => Self::CsvWriter,
            BuiltinType::CsvDictReader => Self::CsvDictReader,
            BuiltinType::CsvDictWriter => Self::CsvDictWriter,
            BuiltinType::CsvDialect => Self::CsvDialect,
            BuiltinType::Decimal => Self::Decimal,
            BuiltinType::DecimalContext => Self::DecimalContext,
            BuiltinType::DecimalSignalDict => Self::DecimalSignalDict,
            BuiltinType::DecimalContextManager => Self::DecimalContextManager,
            BuiltinType::IPv4Address => Self::IPv4Address,
            BuiltinType::IPv6Address => Self::IPv6Address,
            BuiltinType::IPv4Network => Self::IPv4Network,
            BuiltinType::IPv6Network => Self::IPv6Network,
        }
    }
}

impl TryFrom<Type> for BuiltinType {
    type Error = &'static str;

    fn try_from(value: Type) -> Result<Self, Self::Error> {
        Ok(match value {
            Type::Ellipsis => Self::Ellipsis,
            Type::NotImplementedType => Self::NotImplementedType,
            Type::Type => Self::Type,
            Type::NoneType => Self::NoneType,
            Type::Bool => Self::Bool,
            Type::Int => Self::Int,
            Type::Float => Self::Float,
            Type::Complex => Self::Complex,
            Type::Range => Self::Range,
            Type::Slice => Self::Slice,
            Type::Date => Self::Date,
            Type::DateTime => Self::DateTime,
            Type::TimeDelta => Self::TimeDelta,
            Type::TimeZone => Self::TimeZone,
            Type::Str => Self::Str,
            Type::Bytes => Self::Bytes,
            Type::List => Self::List,
            Type::Tuple => Self::Tuple,
            Type::NamedTuple => Self::NamedTuple,
            Type::Dict => Self::Dict,
            Type::DefaultDict => Self::DefaultDict,
            Type::Counter => Self::Counter,
            Type::DictKeys => Self::DictKeys,
            Type::DictItems => Self::DictItems,
            Type::DictValues => Self::DictValues,
            Type::Set => Self::Set,
            Type::FrozenSet => Self::FrozenSet,
            Type::HostClass => Self::HostClass,
            Type::Exception(exc_type) => Self::Exception(exc_type),
            Type::Function => Self::Function,
            Type::BuiltinFunction => Self::BuiltinFunction,
            Type::Cell => Self::Cell,
            Type::Iterator => Self::Iterator,
            Type::ListIterator => Self::ListIterator,
            Type::CallableIterator => Self::CallableIterator,
            Type::Coroutine => Self::Coroutine,
            Type::Module => Self::Module,
            Type::TextIOWrapper => Self::TextIOWrapper,
            Type::BufferedReader => Self::BufferedReader,
            Type::BufferedWriter => Self::BufferedWriter,
            Type::BufferedRandom => Self::BufferedRandom,
            Type::SpecialForm => Self::SpecialForm,
            Type::Path => Self::Path,
            Type::Property => Self::Property,
            Type::RePattern => Self::RePattern,
            Type::ReMatch => Self::ReMatch,
            Type::DirEntry => Self::DirEntry,
            Type::ScandirIterator => Self::ScandirIterator,
            Type::WalkGenerator => Self::WalkGenerator,
            Type::TupleIterator => Self::TupleIterator,
            Type::StrAsciiIterator => Self::StrAsciiIterator,
            Type::StrIterator => Self::StrIterator,
            Type::BytesIterator => Self::BytesIterator,
            Type::RangeIterator => Self::RangeIterator,
            Type::DictKeyIterator => Self::DictKeyIterator,
            Type::DictItemIterator => Self::DictItemIterator,
            Type::DictValueIterator => Self::DictValueIterator,
            Type::SetIterator => Self::SetIterator,
            Type::ItertoolsCount => Self::ItertoolsCount,
            Type::ItertoolsRepeat => Self::ItertoolsRepeat,
            Type::DataclassField => Self::DataclassField,
            Type::Deque => Self::Deque,
            Type::DequeIterator => Self::DequeIterator,
            Type::ItertoolsPairwise => Self::ItertoolsPairwise,
            Type::ItertoolsCompress => Self::ItertoolsCompress,
            Type::ItertoolsIslice => Self::ItertoolsIslice,
            Type::ItertoolsChain => Self::ItertoolsChain,
            Type::ItertoolsCycle => Self::ItertoolsCycle,
            Type::DataclassParams => Self::DataclassParams,
            Type::MissingType => Self::MissingType,
            Type::ItertoolsTakeWhile => Self::ItertoolsTakeWhile,
            Type::ItertoolsDropWhile => Self::ItertoolsDropWhile,
            Type::ItertoolsFilterFalse => Self::ItertoolsFilterFalse,
            Type::ItertoolsStarMap => Self::ItertoolsStarMap,
            Type::Object => Self::Object,
            Type::Time => Self::Time,
            Type::Partial => Self::Partial,
            Type::ItertoolsAccumulate => Self::ItertoolsAccumulate,
            Type::ItertoolsBatched => Self::ItertoolsBatched,
            Type::ItertoolsZipLongest => Self::ItertoolsZipLongest,
            Type::GenericAlias => Self::GenericAlias,
            Type::Union => Self::Union,
            Type::ItertoolsCombinations => Self::ItertoolsCombinations,
            Type::ItertoolsCombinationsWithReplacement => Self::ItertoolsCombinationsWithReplacement,
            Type::ItertoolsPermutations => Self::ItertoolsPermutations,
            Type::ItertoolsProduct => Self::ItertoolsProduct,
            Type::ItertoolsGroupBy => Self::ItertoolsGroupBy,
            Type::ItertoolsGrouper => Self::ItertoolsGrouper,
            Type::ItertoolsTee => Self::ItertoolsTee,
            Type::ItertoolsTeeDataObject => Self::ItertoolsTeeDataObject,
            Type::Random => Self::Random,
            Type::LruCacheWrapper => Self::LruCacheWrapper,
            Type::HashlibHash => Self::HashlibHash,
            Type::HashlibHashXof => Self::HashlibHashXof,
            Type::Blake2b => Self::Blake2b,
            Type::Blake2s => Self::Blake2s,
            Type::CsvReader => Self::CsvReader,
            Type::CsvWriter => Self::CsvWriter,
            Type::CsvDictReader => Self::CsvDictReader,
            Type::CsvDictWriter => Self::CsvDictWriter,
            Type::CsvDialect => Self::CsvDialect,
            Type::Decimal => Self::Decimal,
            Type::DecimalContext => Self::DecimalContext,
            Type::DecimalSignalDict => Self::DecimalSignalDict,
            Type::DecimalContextManager => Self::DecimalContextManager,
            Type::IPv4Address => Self::IPv4Address,
            Type::IPv6Address => Self::IPv6Address,
            Type::IPv4Network => Self::IPv4Network,
            Type::IPv6Network => Self::IPv6Network,
            Type::Instance(_) => return Err("user-defined classes use heap type objects"),
        })
    }
}

impl fmt::Display for BuiltinType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Type::from(*self).fmt(f)
    }
}

#[cfg(test)]
mod builtin_type_tests {
    use std::mem::size_of;

    use strum::VariantNames;

    use super::{BuiltinType, ExcType, Type};
    use crate::{builtins::Builtins, heap::HeapId, value::Value};

    #[test]
    fn builtin_type_names_match_runtime_types() {
        let runtime_names: Vec<_> = Type::VARIANTS
            .iter()
            .copied()
            .filter(|name| *name != "instance")
            .collect();
        assert_eq!(BuiltinType::VARIANTS, runtime_names);

        for name in runtime_names.into_iter().filter(|name| *name != "exception") {
            let builtin: BuiltinType = name.parse().unwrap();
            let runtime: Type = name.parse().unwrap();
            assert_eq!(Type::from(builtin), runtime, "{name}");

            let builtin_name: &'static str = builtin.into();
            let runtime_name: &'static str = runtime.into();
            assert_eq!(builtin_name, runtime_name, "{name}");
        }
    }

    #[test]
    fn static_type_conversions_preserve_serialization() {
        let types = Type::VARIANTS
            .iter()
            .filter(|name| !matches!(**name, "instance" | "exception"))
            .map(|name| name.parse::<Type>().unwrap());
        let exceptions = ExcType::VARIANTS
            .iter()
            .map(|name| Type::Exception(name.parse().unwrap()));
        assert_eq!(BuiltinType::VARIANTS.len() + 1, Type::VARIANTS.len());

        for ty in types.chain(exceptions) {
            let builtin = BuiltinType::try_from(ty).unwrap();
            assert_eq!(Type::from(builtin), ty);
            assert_eq!(builtin.to_string(), ty.to_string());

            let cbor = minicbor_serde::to_vec(ty).unwrap();
            assert_eq!(minicbor_serde::to_vec(builtin).unwrap(), cbor);
            assert_eq!(minicbor_serde::from_slice::<BuiltinType>(&cbor).unwrap(), builtin);

            // id() encodes builtins through postcard, whose variant indices must also stay stable.
            let postcard = postcard::to_allocvec(&ty).unwrap();
            assert_eq!(postcard::to_allocvec(&builtin).unwrap(), postcard);
            assert_eq!(postcard::from_bytes::<BuiltinType>(&postcard).unwrap(), builtin);
        }
    }

    // Captured from Type before the split. Updating this fixture requires checking
    // dump compatibility and id() stability, not just regenerating expected output.
    #[test]
    fn builtin_type_serialization_matches_legacy_format() {
        let fixture = include_str!("../../tests/fixtures/builtin_type_serialization.txt");
        assert_eq!(fixture.lines().count() + 1, BuiltinType::VARIANTS.len());

        for line in fixture.lines() {
            let (name, index) = line.split_once(' ').unwrap();
            let cbor = minicbor_serde::to_vec(name).unwrap();
            let builtin: BuiltinType = minicbor_serde::from_slice(&cbor).unwrap();

            assert_eq!(minicbor_serde::to_vec(builtin).unwrap(), cbor, "{name}");
            let postcard = vec![index.parse::<u8>().unwrap()];
            assert_eq!(postcard::to_allocvec(&builtin).unwrap(), postcard, "{name}");
            assert_eq!(postcard::from_bytes::<BuiltinType>(&postcard).unwrap(), builtin);
        }

        // Exception carries a payload and follows the removed Instance slot.
        let cbor = b"\xa1\x69Exception\x6aValueError";
        let builtin = BuiltinType::Exception(ExcType::ValueError);
        assert_eq!(minicbor_serde::to_vec(builtin).unwrap(), cbor);
        assert_eq!(minicbor_serde::from_slice::<BuiltinType>(cbor).unwrap(), builtin);
        let postcard = [29, 17];
        assert_eq!(postcard::to_allocvec(&builtin).unwrap(), postcard);
        assert_eq!(postcard::from_bytes::<BuiltinType>(&postcard).unwrap(), builtin);
    }

    #[test]
    fn instance_types_cannot_be_builtins() {
        let instance = Type::Instance(HeapId::from_index(0));
        assert!(BuiltinType::try_from(instance).is_err());

        let cbor = minicbor_serde::to_vec(instance).unwrap();
        assert!(minicbor_serde::from_slice::<BuiltinType>(&cbor).is_err());

        let postcard = postcard::to_allocvec(&instance).unwrap();
        assert!(postcard::from_bytes::<BuiltinType>(&postcard).is_err());
    }

    #[test]
    fn compact_builtin_layout() {
        assert_eq!(size_of::<BuiltinType>(), 1);
        assert_eq!(size_of::<Builtins>(), 2);

        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<Value>(), 16);
    }
}
