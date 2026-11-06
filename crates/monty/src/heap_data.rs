use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
    ops::Deref,
};

use monty_types::ExcType;

use crate::{
    annotations,
    args::ArgValues,
    asyncio::{Awaiter, Coroutine, ExternalFuture, ExternalFutureState, GatherFuture, GatherState},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::FunctionId,
    types::{LazyHeapSet, LongInt, PyTrait, Type, TypeRef},
    value::{EitherStr, Value},
};

macro_rules! heap_storage_type {
    (inline $payload:ty) => {
        $payload
    };
    (boxed $payload:ty) => {
        Box<$payload>
    };
}

/// Invokes a consumer macro with every concrete payload stored by the heap.
///
/// This is the single payload registry. Dumps tag each variant by its serde name, so
/// renaming a variant needs an alias; hot variants take one letter to keep dumps small.
macro_rules! heap_payloads {
    ($consumer:ident) => {
        $consumer! {
            #[serde(rename = "S")]
            Str(inline $crate::types::Str),
            #[serde(rename = "B")]
            Bytes(inline $crate::types::Bytes),
            #[serde(rename = "L")]
            List(inline $crate::types::List),
            /// `collections.deque` — a double-ended queue with an optional `maxlen`.
            #[serde(rename = "Q")]
            Deque(inline $crate::types::Deque),
            #[serde(rename = "T")]
            Tuple(inline $crate::types::Tuple),
            #[serde(rename = "N")]
            NamedTuple(boxed $crate::types::NamedTuple),
            /// A `collections.namedtuple` class object (the callable that builds instances).
            NamedTupleClass(boxed $crate::types::NamedTupleClass),
            #[serde(rename = "D")]
            Dict(inline $crate::types::Dict),
            DictKeysView(inline $crate::types::DictKeysView),
            DictItemsView(inline $crate::types::DictItemsView),
            DictValuesView(inline $crate::types::DictValuesView),
            #[serde(rename = "E")]
            Set(inline $crate::types::Set),
            #[serde(rename = "F")]
            FrozenSet(inline $crate::types::FrozenSet),
            #[serde(rename = "C")]
            Closure(inline $crate::heap_data::Closure),
            #[serde(rename = "U")]
            FunctionDefaults(inline $crate::heap_data::FunctionDefaults),
            /// A cell wrapping a single mutable value for closure support.
            #[serde(rename = "K")]
            Cell(inline $crate::heap_data::CellValue),
            /// A range object such as `range(1, 10, 2)`.
            Range(inline $crate::types::Range),
            /// A slice object such as `slice(1, 10, 2)`.
            Slice(inline $crate::types::Slice),
            /// An exception instance such as `ValueError('message')`.
            #[serde(rename = "X")]
            Exception(inline $crate::exception_private::SimpleException),
            /// A host-backed class instance (the heap form of the wire `ClassInstance`).
            HostClass(boxed $crate::types::HostClass),
            /// The lightweight type object `type(x)` materializes for a `HostClass`
            /// instance — the real class lives on the host, so this is a named
            /// stand-in (see `HostClassType`).
            HostClassType(boxed $crate::types::HostClassType),
            /// A user-defined class object created by a `class` statement.
            #[serde(rename = "A")]
            Class(boxed $crate::types::Class),
            /// An instance of a user-defined class.
            #[serde(rename = "I")]
            Instance(boxed $crate::types::Instance),
            /// A method bound to an instance.
            #[serde(rename = "M")]
            BoundMethod(inline $crate::types::BoundMethod),
            /// One `dataclasses.Field` held by a class's `__dataclass_fields__` dictionary.
            /// Boxed: a name, an annotation and a default or factory are already
            /// wider than the 32-bit ceiling, and `Field` has attributes left to gain.
            DataclassField(boxed $crate::modules::dataclasses::DataclassField),
            /// A method `@dataclass` generated into a class namespace.
            /// Boxed: its `__init__` form carries every field's captured default.
            DataclassMethod(boxed $crate::modules::dataclasses::DataclassMethod),
            /// A `list_iterator` object.
            ListIterator(inline $crate::types::list::ListIterator),
            /// A `_collections._deque_iterator` object.
            DequeIterator(inline $crate::types::deque::DequeIterator),
            /// A `tuple_iterator` object.
            TupleIterator(inline $crate::types::TupleIterator),
            /// A `str_ascii_iterator` or `str_iterator` object.
            StringIterator(inline $crate::types::StringIterator),
            /// A `bytes_iterator` object.
            BytesIterator(inline $crate::types::BytesIterator),
            /// A `range_iterator` object.
            RangeIterator(inline $crate::types::RangeIterator),
            /// A `dict_keyiterator` object.
            DictKeyIterator(inline $crate::types::DictKeyIterator),
            /// A `dict_itemiterator` object.
            DictItemIterator(inline $crate::types::DictItemIterator),
            /// A `dict_valueiterator` object.
            DictValueIterator(inline $crate::types::DictValueIterator),
            /// A `set_iterator` object.
            SetIterator(inline $crate::types::SetIterator),
            /// A `callable_iterator` object from `iter(callable, sentinel)`.
            CallableIterator(inline $crate::types::callable_iterator::CallableIterator),
            /// An arbitrary-precision integer used when a Python `int` does not fit in `i64`.
            #[serde(rename = "G")]
            LongInt(inline $crate::types::LongInt),
            /// A `complex` number: two doubles, too wide for an immediate `Value`.
            #[serde(rename = "J")]
            Complex(inline $crate::types::Complex),
            /// A Python module and its attributes.
            Module(boxed $crate::types::Module),
            /// A coroutine object from an async function call.
            #[serde(rename = "O")]
            Coroutine(inline $crate::asyncio::Coroutine),
            /// An `asyncio.gather()` result tracking multiple coroutines or tasks.
            GatherFuture(boxed $crate::asyncio::GatherFuture),
            /// An external future driven by the host.
            ExternalFuture(boxed $crate::asyncio::ExternalFuture),
            /// A filesystem path from `pathlib.Path`.
            Path(inline $crate::types::Path),
            /// A path-backed file object returned by `open()`.
            OpenFile(boxed $crate::types::OpenFile),
            /// A compiled regular-expression pattern.
            RePattern(boxed $crate::types::RePattern),
            /// A regular-expression match result.
            ReMatch(boxed $crate::types::ReMatch),
            /// A reference to an external function supplied by the host.
            ExtFunction(inline $crate::types::ExtFunction),
            /// A `datetime.date` value.
            Date(inline $crate::types::date::Date),
            /// A `datetime.datetime` value.
            DateTime(inline $crate::types::datetime::DateTime),
            /// A `datetime.timedelta` value.
            TimeDelta(inline $crate::types::timedelta::TimeDelta),
            /// A fixed-offset `datetime.timezone` value.
            TimeZone(inline $crate::types::timezone::TimeZone),
            /// Any `itertools` iterator (`count`, `repeat`, and others).
            Itertools(inline $crate::types::ItertoolsIter),
            /// The options of a `@dataclass`, held in `__dataclass_params__`.
            DataclassParams(inline $crate::modules::dataclasses::DataclassParams),
            /// A `datetime.time` value stored with narrow integer fields.
            Time(inline $crate::types::time::Time),
            /// A `functools.partial` object.
            Partial(boxed $crate::types::Partial),
            /// A `types.GenericAlias` such as `list[int]`.
            GenericAlias(inline $crate::types::GenericAlias),
            /// A `typing.Union` such as `int | None`.
            Union(inline $crate::types::Union),
            /// An `annotationlib.ForwardRef`, also the stringifier a `FORWARDREF` evaluation composes.
            ForwardRef(boxed $crate::types::ForwardRef),
            /// A `random.Random` generator instance.
            Random(boxed $crate::types::Random),
            /// A `functools.lru_cache` / `functools.cache` wrapper.
            LruCache(boxed $crate::types::LruCache),
            /// A `hashlib` hash object mid-stream.
            HashObject(boxed $crate::types::HashObject),
            /// An `os.DirEntry`, `os.scandir()` iterator or `os.walk()` iterator.
            DirScan(boxed $crate::types::DirScan),
            /// Any `csv` object: a reader, writer, `DictReader`, `DictWriter` or dialect.
            Csv(boxed $crate::types::CsvObject),
            /// A `decimal.Decimal` value.
            Decimal(inline $crate::types::decimal::Decimal),
            /// A mutable `decimal.Context`.
            DecimalContext(inline $crate::types::decimal::DecContext),
            /// A context's `flags` or `traps` mapping.
            DecimalSignalDict(inline $crate::types::decimal::py_context::SignalDict),
            /// The manager `decimal.localcontext()` returns.
            DecimalContextManager(inline $crate::types::decimal::py_context::ContextManager),
        }
    };
}

pub(crate) use heap_payloads;

macro_rules! define_heap_data {
    ($(
        $(#[$meta:meta])*
        $variant:ident($storage:ident $payload:ty)
    ),* $(,)?) => {
        /// Every runtime value that can be stored in the heap arena.
        #[derive(Debug, serde::Serialize, serde::Deserialize)]
        pub(crate) enum HeapData {
            $(
                $(#[$meta])*
                $variant(heap_storage_type!($storage $payload)),
            )*
        }
    };
}

heap_payloads!(define_heap_data);

/// The widest `HeapData` may be, per target.
///
/// `Dict` sets it on 64-bit hosts. On 32-bit (the wasm worker) `Dict` halves
/// while `Value`-built payloads do not, so the itertools family sets it instead
/// — see the budget those adaptors are held to in `types::itertools`.
const MAX_HEAP_DATA_SIZE: usize = if cfg!(target_pointer_width = "64") { 80 } else { 56 };

// `HeapData` is copied on every allocate and free, and every byte past the
// ceiling widens every slot of every heap page. Box the variant that trips
// this, not the enum: growing the budget here charges the hot variants too.
const _: () = assert!(mem::size_of::<HeapData>() <= MAX_HEAP_DATA_SIZE);

impl HeapData {
    /// Returns whether this heap data type can participate in reference cycles.
    ///
    /// Only container types that can hold references to other heap objects need to be
    /// tracked for GC purposes. Leaf types like Str, Bytes, Range, and Exception cannot
    /// form cycles and should not count toward the GC allocation threshold.
    ///
    /// This optimization allows programs that allocate many leaf objects (like strings)
    /// to avoid triggering unnecessary GC cycles.
    ///
    /// Matched exhaustively so new variants must choose.
    #[inline]
    pub(crate) fn is_gc_tracked(&self) -> bool {
        match self {
            Self::Itertools(iter) => iter.is_gc_tracked(),
            Self::DirScan(scan) => scan.is_gc_tracked(),
            Self::List(_)
            | Self::Deque(_)
            | Self::Tuple(_)
            | Self::NamedTuple(_)
            | Self::NamedTupleClass(_)
            | Self::Dict(_)
            | Self::DictKeysView(_)
            | Self::DictItemsView(_)
            | Self::DictValuesView(_)
            | Self::Set(_)
            | Self::FrozenSet(_)
            | Self::Closure(_)
            | Self::FunctionDefaults(_)
            | Self::Cell(_)
            | Self::HostClass(_)
            | Self::HostClassType(_)
            | Self::Class(_)
            | Self::Instance(_)
            | Self::BoundMethod(_)
            | Self::DataclassField(_)
            | Self::DataclassMethod(_)
            | Self::ListIterator(_)
            | Self::DequeIterator(_)
            | Self::TupleIterator(_)
            | Self::DictKeyIterator(_)
            | Self::DictItemIterator(_)
            | Self::DictValueIterator(_)
            | Self::SetIterator(_)
            | Self::CallableIterator(_)
            | Self::Module(_)
            | Self::Coroutine(_)
            | Self::GatherFuture(_)
            | Self::ExternalFuture(_)
            | Self::Partial(_)
            | Self::GenericAlias(_)
            | Self::Union(_)
            | Self::ForwardRef(_)
            | Self::LruCache(_)
            | Self::Csv(_) => true,
            // Leaf types, plus iterators whose heap refs only point at leaves and so
            // cannot close a cycle. Move one up if it gains a container-valued field.
            Self::Str(_)
            | Self::Bytes(_)
            | Self::Range(_)
            | Self::Slice(_)
            | Self::Exception(_)
            | Self::DataclassParams(_)
            | Self::StringIterator(_)
            | Self::BytesIterator(_)
            | Self::RangeIterator(_)
            | Self::LongInt(_)
            | Self::Complex(_)
            | Self::Path(_)
            | Self::OpenFile(_)
            | Self::RePattern(_)
            | Self::ReMatch(_)
            | Self::ExtFunction(_)
            | Self::Date(_)
            | Self::DateTime(_)
            | Self::Time(_)
            | Self::TimeDelta(_)
            | Self::TimeZone(_)
            | Self::Random(_)
            | Self::HashObject(_)
            // A signal dict and a `localcontext()` manager only hold contexts, which are leaves.
            | Self::Decimal(_)
            | Self::DecimalContext(_)
            | Self::DecimalSignalDict(_)
            | Self::DecimalContextManager(_) => false,
        }
    }

    /// Whether calling a `Ref` to this heap data would succeed at dispatch.
    ///
    /// A conservative subset of the types overriding [`PyTrait::py_call`]: it
    /// is what `partial()` and friends screen a callable argument with, and it
    /// has never admitted `HostClassType` or `NamedTupleClass`, both of which
    /// dispatch perfectly well. Widening it changes what those builtins accept,
    /// so it is not simply the list of `py_call` overrides.
    #[must_use]
    pub(crate) fn is_callable(&self) -> bool {
        matches!(
            self,
            Self::Class(_)
                | Self::BoundMethod(_)
                | Self::DataclassMethod(_)
                | Self::Closure(_)
                | Self::FunctionDefaults(_)
                | Self::ExtFunction(_)
                | Self::Partial(_)
                | Self::GenericAlias(_)
                | Self::LruCache(_)
        )
    }

    /// Returns the Python type of this heap data without requiring VM access.
    ///
    /// This is a lightweight alternative to the `PyTrait::py_type` dispatch on
    /// `HeapReadOutput`, useful in error messages and diagnostics where only a
    /// `&Heap` is available (not a full `&VM`).
    #[must_use]
    pub(crate) fn py_type(&self) -> TypeRef {
        TypeRef::Native(match self {
            Self::Str(_) => Type::Str,
            Self::Bytes(_) => Type::Bytes,
            Self::List(_) => Type::List,
            Self::Deque(_) => Type::Deque,
            Self::Tuple(_) | Self::NamedTuple(_) => Type::Tuple,
            Self::NamedTupleClass(_) => Type::Type,
            Self::Dict(_) => Type::Dict,
            Self::Partial(_) => Type::Partial,
            Self::Random(_) => Type::Random,
            Self::HashObject(hash) => hash.algorithm().py_type(),
            Self::GenericAlias(_) => Type::GenericAlias,
            Self::Union(_) => Type::Union,
            Self::ForwardRef(_) => Type::ForwardRef,
            Self::LruCache(_) => Type::LruCacheWrapper,
            Self::DictKeysView(_) => Type::DictKeys,
            Self::DictItemsView(_) => Type::DictItems,
            Self::DictValuesView(_) => Type::DictValues,
            Self::Set(_) => Type::Set,
            Self::FrozenSet(_) => Type::FrozenSet,
            Self::Closure(_) | Self::FunctionDefaults(_) | Self::ExtFunction(_) => Type::Function,
            Self::Cell(_) => Type::Cell,
            Self::Range(_) => Type::Range,
            Self::Slice(_) => Type::Slice,
            Self::Exception(e) => Type::from(e.exc_type()),
            // An instance's type is its class object.
            Self::HostClass(instance) => return TypeRef::Class(instance.class_id()),
            Self::Instance(instance) => return TypeRef::Class(instance.class()),
            // A class object's type is `type`.
            Self::HostClassType(_) | Self::Class(_) => Type::Type,
            Self::BoundMethod(_) => Type::Function,
            Self::DataclassField(_) => Type::DataclassField,
            Self::DataclassMethod(_) => Type::Function,
            Self::DataclassParams(_) => Type::DataclassParams,
            Self::LongInt(_) => Type::Int,
            Self::Complex(_) => Type::Complex,
            Self::Module(_) => Type::Module,
            Self::Coroutine(_) | Self::GatherFuture(_) | Self::ExternalFuture(_) => Type::Coroutine,
            Self::Path(_) => Type::Path,
            Self::OpenFile(file) => file.file_type(),
            Self::RePattern(_) => Type::RePattern,
            Self::ReMatch(_) => Type::ReMatch,
            Self::Date(_) => Type::Date,
            Self::DateTime(_) => Type::DateTime,
            Self::Time(_) => Type::Time,
            Self::TimeDelta(_) => Type::TimeDelta,
            Self::TimeZone(_) => Type::TimeZone,
            Self::ListIterator(_) => Type::ListIterator,
            Self::DequeIterator(_) => Type::DequeIterator,
            Self::TupleIterator(_) => Type::TupleIterator,
            Self::StringIterator(iter) => iter.py_type(),
            Self::BytesIterator(_) => Type::BytesIterator,
            Self::RangeIterator(_) => Type::RangeIterator,
            Self::DictKeyIterator(_) => Type::DictKeyIterator,
            Self::DictItemIterator(_) => Type::DictItemIterator,
            Self::DictValueIterator(_) => Type::DictValueIterator,
            Self::SetIterator(_) => Type::SetIterator,
            Self::CallableIterator(_) => Type::CallableIterator,
            Self::Itertools(i) => i.py_type(),
            Self::DirScan(scan) => scan.py_type(),
            Self::Csv(csv) => csv.py_type(),
            Self::Decimal(_) => Type::Decimal,
            Self::DecimalContext(_) => Type::DecimalContext,
            Self::DecimalSignalDict(_) => Type::DecimalSignalDict,
            Self::DecimalContextManager(_) => Type::DecimalContextManager,
        })
    }
}

/// Thin wrapper around `Value` which is used in the `Cell` variant above.
///
/// The inner value is the cell's mutable payload.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub(crate) struct CellValue(pub(crate) Value);

impl Deref for CellValue {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A closure: a function that captures variables from enclosing scopes.
///
/// Contains a reference to the function definition, a vector of captured cell HeapIds,
/// evaluated default values (if any) and the globals dict it was defined under
/// (if any). When the closure is called, the cells are passed to the frame for
/// variable access. When the closure is dropped, we must decrement the ref
/// count on each captured cell, each default value and the globals dict.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Closure {
    /// The function definition being captured.
    #[serde(rename = "F")]
    pub func_id: FunctionId,
    /// Captured cells from enclosing scopes. Boxed slices rather than `Vec`s
    /// (never grown after construction) keep this the size of the largest
    /// `HeapData` variant, not larger.
    #[serde(rename = "C")]
    pub cells: Box<[HeapId]>,
    /// Evaluated default parameter values (if any).
    #[serde(rename = "D")]
    pub defaults: Box<[Value]>,
    /// Owned reference to the `exec()` / `eval()` globals dict the closure was
    /// defined under; `None` when its globals are module slots.
    #[serde(rename = "G")]
    pub globals: Option<HeapId>,
}

/// A `def` that needs a heap object but captures nothing: it has evaluated
/// default values, an explicit globals dict, or both.
///
/// When the function is called, defaults are cloned for missing optional
/// parameters and the frame resolves globals through the dict. When dropped,
/// we must decrement the ref count on each default value and the dict.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct FunctionDefaults {
    /// The function definition being captured.
    #[serde(rename = "F")]
    pub func_id: FunctionId,
    /// Evaluated default parameter values (if any).
    #[serde(rename = "D")]
    pub defaults: Vec<Value>,
    /// Owned reference to the `exec()` / `eval()` globals dict the function
    /// was defined under; `None` when its globals are module slots.
    #[serde(rename = "G")]
    pub globals: Option<HeapId>,
}

impl HeapItem for CellValue {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.0.py_dec_ref_ids(stack);
    }
}

impl HeapItem for Closure {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for captured cells
        stack.extend(self.cells.iter().copied());
        // Decrement ref count for default values that are heap references
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
        stack.extend(self.globals);
    }
}

impl HeapItem for FunctionDefaults {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for default values that are heap references
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
        stack.extend(self.globals);
    }
}

/// The shared tail of calling a `def`: both function representations differ only
/// in whether they carry captured cells.
///
/// The cells and defaults are copied out before dispatching, so no borrow on the
/// callable is live while its body runs and the body may reach it again.
fn call_def(
    func_id: FunctionId,
    cells: &[HeapId],
    defaults: Vec<Value>,
    globals: Option<HeapId>,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    defer_drop!(defaults, vm);
    vm.call_def_function(func_id, cells, defaults, globals, args)
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Closure> {
    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::Function.into()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Two closures over the same `def` are equal only if they captured the
    /// same cells; the defaults play no part, as in CPython.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(match other.read_heap(vm) {
            Some(HeapReadOutput::Closure(other)) => {
                let this = self.get(vm.heap);
                let other = other.get(vm.heap);
                Some(this.func_id == other.func_id && this.cells == other.cells)
            }
            _ => None,
        })
    }

    /// Hashes by `def`, so a closure and its equal share a bucket.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(hash_func_id(self.get(vm.heap).func_id)))
    }

    fn py_bool(&self, _: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        let func_id = self.get(vm.heap).func_id;
        Ok(vm.interns.get_function(func_id).py_repr_fmt(f, vm.interns, 0)?)
    }

    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let closure = self.get(vm.heap);
        let (func_id, cells, globals) = (closure.func_id, closure.cells.clone(), closure.globals);
        let defaults = closure.defaults.iter().map(|v| v.clone_with_heap(vm)).collect();
        call_def(func_id, &cells, defaults, globals, args, vm)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let closure = self.get(vm.heap);
        let (func_id, globals) = (closure.func_id, closure.globals);
        annotations::function_getattr(func_id, globals, attr.as_str(vm.interns), vm).transpose()
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let closure = self.get(vm.heap);
        let (func_id, globals) = (closure.func_id, closure.globals);
        annotations::function_call_attr(func_id, globals, attr.as_str(vm.interns), args, vm)
            .unwrap_or_else(|| Err(ExcType::attribute_error("function", attr.as_str(vm.interns))))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, FunctionDefaults> {
    fn py_type(&self, _: &VM<'h>) -> TypeRef {
        Type::Function.into()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Equal when they decorate the same `def`: with no captured scope, the
    /// defaults are all that could differ and CPython ignores those too.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(match other.read_heap(vm) {
            Some(HeapReadOutput::FunctionDefaults(other)) => {
                Some(self.get(vm.heap).func_id == other.get(vm.heap).func_id)
            }
            _ => None,
        })
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(hash_func_id(self.get(vm.heap).func_id)))
    }

    fn py_bool(&self, _: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        let func_id = self.get(vm.heap).func_id;
        Ok(vm.interns.get_function(func_id).py_repr_fmt(f, vm.interns, 0)?)
    }

    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let function = self.get(vm.heap);
        let (func_id, globals) = (function.func_id, function.globals);
        let defaults = function.defaults.iter().map(|v| v.clone_with_heap(vm)).collect();
        call_def(func_id, &[], defaults, globals, args, vm)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let function = self.get(vm.heap);
        let (func_id, globals) = (function.func_id, function.globals);
        annotations::function_getattr(func_id, globals, attr.as_str(vm.interns), vm).transpose()
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let function = self.get(vm.heap);
        let (func_id, globals) = (function.func_id, function.globals);
        annotations::function_call_attr(func_id, globals, attr.as_str(vm.interns), args, vm)
            .unwrap_or_else(|| Err(ExcType::attribute_error("function", attr.as_str(vm.interns))))
    }
}

/// Hashes a function identity, the hash both `def` representations report.
fn hash_func_id(func_id: FunctionId) -> HashValue {
    let mut hasher = DefaultHasher::new();
    func_id.hash(&mut hasher);
    HashValue::new(hasher.finish())
}

impl HeapItem for SimpleException {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Only the chain holds references; they are always exceptions, so an
        // exception still cannot reach a container and stays a GC leaf.
        stack.extend(self.owned_chain_ids());
    }
}

impl HeapItem for LongInt {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // LongInt doesn't contain heap references
    }
}

impl HeapItem for Coroutine {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for namespace values that are heap references
        for value in &mut self.namespace {
            value.py_dec_ref_ids(stack);
        }
        stack.extend(self.globals);
    }
}

impl HeapItem for GatherFuture {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Decrement ref count for items the gather owns (every entry in
        // `items` is inc_ref'd at construction time).
        stack.extend(self.items.iter().copied());
        // Release per-state heap refs: in-flight slot results plus this
        // gather's own awaiter (if `GatherSlot`, it owns an inc_ref on the
        // outer gather), or the cached result list once the gather has
        // completed successfully. `Pending` and `Failed` carry no heap refs.
        match &mut self.state {
            GatherState::Awaited(awaited) => {
                if let Awaiter::GatherSlot { gather, .. } = &awaited.awaiter {
                    stack.push(*gather);
                }
                for result in awaited.results.iter_mut().flatten() {
                    result.py_dec_ref_ids(stack);
                }
            }
            GatherState::Completed(Value::Ref(id)) => stack.push(*id),
            GatherState::Pending | GatherState::Failed(_) | GatherState::Completed(_) => {}
        }
    }
}

impl HeapItem for ExternalFuture {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // `Pending { awaiter: Some(Awaiter::GatherSlot { gather, .. }) }`
        // owns an inc_ref on `gather` — release it when this entry is
        // freed. `Awaiter::Task` and `None` own nothing. `Resolved` owns
        // the cached value; `Failed` carries no heap refs. A pending sleep
        // result is owned until resolution takes it.
        if let Some(result) = &mut self.sleep_result {
            result.py_dec_ref_ids(stack);
        }
        match &mut self.state {
            ExternalFutureState::Resolved(value) => value.py_dec_ref_ids(stack),
            ExternalFutureState::Pending {
                awaiter: Some(Awaiter::GatherSlot { gather, .. }),
            } => stack.push(*gather),
            ExternalFutureState::Pending {
                awaiter: None | Some(Awaiter::Task(_)),
            }
            | ExternalFutureState::Failed(_) => {}
        }
    }
}
