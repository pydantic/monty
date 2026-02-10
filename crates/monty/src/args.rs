use std::vec::IntoIter;

use crate::{
    MontyObject, ResourceTracker, defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    expressions::{ExprLoc, Identifier},
    heap::{DropWithHeap, Heap, HeapGuard},
    intern::{Interns, StringId},
    parse::ParseError,
    types::{Dict, dict::DictIntoIter},
    value::Value,
};

/// Type for method call arguments.
///
/// Uses specific variants for common cases (0-2 arguments).
/// Most Python method calls have at most 2 arguments, so this optimization
/// eliminates the Vec heap allocation overhead for the vast majority of calls.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum ArgValues {
    Empty,
    One(Value),
    Two(Value, Value),
    Kwargs(KwargsValues),
    ArgsKargs { args: Vec<Value>, kwargs: KwargsValues },
}

impl ArgValues {
    /// Checks that zero arguments were passed.
    ///
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn check_zero_args(self, name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<()> {
        match self {
            Self::Empty => Ok(()),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                Err(ExcType::type_error_no_args(name, count))
            }
        }
    }

    /// Checks that exactly one positional argument was passed, returning it.
    ///
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn get_one_arg(self, name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Value> {
        match self {
            Self::One(a) => Ok(a),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                Err(ExcType::type_error_arg_count(name, 1, count))
            }
        }
    }

    /// Checks that exactly two positional arguments were passed, returning them as a tuple.
    ///
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn get_two_args(self, name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<(Value, Value)> {
        match self {
            Self::Two(a1, a2) => Ok((a1, a2)),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                Err(ExcType::type_error_arg_count(name, 2, count))
            }
        }
    }

    /// Checks that one or two arguments were passed, returning them as a tuple.
    ///
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn get_one_two_args(
        self,
        name: &str,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> RunResult<(Value, Option<Value>)> {
        match self {
            Self::One(a) => Ok((a, None)),
            Self::Two(a1, a2) => Ok((a1, Some(a2))),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                if count == 0 {
                    Err(ExcType::type_error_at_least(name, 1, count))
                } else {
                    Err(ExcType::type_error_at_most(name, 2, count))
                }
            }
        }
    }

    /// Checks that zero or one argument was passed, returning the optional value.
    ///
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn get_zero_one_arg(self, name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<Option<Value>> {
        match self {
            Self::Empty => Ok(None),
            Self::One(a) => Ok(Some(a)),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                Err(ExcType::type_error_at_most(name, 1, count))
            }
        }
    }

    /// Checks that zero, one, or two arguments were passed.
    ///
    /// Returns (None, None) for 0 args, (Some(a), None) for 1 arg, (Some(a), Some(b)) for 2 args.
    /// On error, properly drops all contained values to maintain reference counts.
    pub fn get_zero_one_two_args(
        self,
        name: &str,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> RunResult<(Option<Value>, Option<Value>)> {
        match self {
            Self::Empty => Ok((None, None)),
            Self::One(a) => Ok((Some(a), None)),
            Self::Two(a, b) => Ok((Some(a), Some(b))),
            other => {
                let count = other.count();
                other.drop_with_heap(heap);
                Err(ExcType::type_error_at_most(name, 2, count))
            }
        }
    }

    /// Extracts two keyword-only arguments by name.
    ///
    /// Validates that no positional arguments are provided and only the specified
    /// keyword arguments are present. Returns `(None, None)` for missing kwargs.
    ///
    /// # Arguments
    /// * `method_name` - Method name for error messages (e.g., "list.sort")
    /// * `kwarg1` - Name of the first keyword argument
    /// * `kwarg2` - Name of the second keyword argument
    ///
    /// # Errors
    /// Returns an error if:
    /// - Any positional arguments are provided
    /// - A keyword argument other than `kwarg1` or `kwarg2` is provided
    /// - A keyword is not a string
    pub fn extract_two_kwargs_only(
        self,
        method_name: &str,
        kwarg1: &str,
        kwarg2: &str,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<(Option<Value>, Option<Value>)> {
        let (pos, kwargs) = self.into_parts();
        defer_drop!(pos, heap);
        let kwargs = kwargs.into_iter();
        defer_drop_mut!(kwargs, heap);

        // Check no positional arguments
        if pos.len() > 0 {
            return Err(ExcType::type_error_no_args(method_name, 1));
        }

        // Parse keyword arguments
        // Guards are reversed so that destructure can pull them
        let mut val2_guard = HeapGuard::new(None, heap);
        let (val2, heap) = val2_guard.as_parts_mut();
        let mut val1_guard = HeapGuard::new(None, heap);
        let (val1, heap) = val1_guard.as_parts_mut();

        for (key, value) in kwargs {
            defer_drop!(key, heap);
            let mut value = HeapGuard::new(value, heap);

            let Some(keyword_name) = key.as_either_str(value.heap()) else {
                return Err(ExcType::type_error("keywords must be strings"));
            };

            let key_str = keyword_name.as_str(interns);
            let old = if key_str == kwarg1 {
                val1.replace(value.into_inner())
            } else if key_str == kwarg2 {
                val2.replace(value.into_inner())
            } else {
                return Err(ExcType::type_error(format!(
                    "'{key_str}' is an invalid keyword argument for {method_name}()"
                )));
            };

            old.drop_with_heap(heap);
        }

        Ok((val1_guard.into_inner(), val2_guard.into_inner()))
    }

    /// Splits into positional iterator and keyword values without allocating
    /// for the common One/Two cases.
    pub fn into_parts(self) -> (ArgPosIter, KwargsValues) {
        match self {
            Self::Empty => (ArgPosIter::Empty, KwargsValues::Empty),
            Self::One(v) => (ArgPosIter::One(v), KwargsValues::Empty),
            Self::Two(v1, v2) => (ArgPosIter::Two([v1, v2]), KwargsValues::Empty),
            Self::Kwargs(kwargs) => (ArgPosIter::Empty, kwargs),
            Self::ArgsKargs { args, kwargs } => (ArgPosIter::Vec(args.into_iter()), kwargs),
        }
    }

    /// Variant of [`into_parts()`](Self::into_parts) that accepts no kwargs, returning an error if any are present.
    pub fn into_pos_only(self, method_name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<ArgPosIter> {
        match self {
            Self::Empty => Ok(ArgPosIter::Empty),
            Self::One(v) => Ok(ArgPosIter::One(v)),
            Self::Two(v1, v2) => Ok(ArgPosIter::Two([v1, v2])),
            Self::Kwargs(kwargs) => {
                if kwargs.is_empty() {
                    Ok(ArgPosIter::Empty)
                } else {
                    Err(Self::unexpected_kwargs_error(kwargs, method_name, heap))
                }
            }
            Self::ArgsKargs { args, kwargs } => {
                if kwargs.is_empty() {
                    Ok(ArgPosIter::Vec(args.into_iter()))
                } else {
                    args.drop_with_heap(heap);
                    Err(Self::unexpected_kwargs_error(kwargs, method_name, heap))
                }
            }
        }
    }

    #[cold]
    fn unexpected_kwargs_error(
        kwargs: KwargsValues,
        method_name: &str,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> RunError {
        kwargs.drop_with_heap(heap);
        ExcType::type_error_no_kwargs(method_name)
    }

    /// Converts the arguments into a Vec of MontyObjects.
    ///
    /// This is used when passing arguments to external functions.
    pub fn into_py_objects(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> (Vec<MontyObject>, Vec<(MontyObject, MontyObject)>) {
        match self {
            Self::Empty => (vec![], vec![]),
            Self::One(a) => (vec![MontyObject::new(a, heap, interns)], vec![]),
            Self::Two(a1, a2) => (
                vec![MontyObject::new(a1, heap, interns), MontyObject::new(a2, heap, interns)],
                vec![],
            ),
            Self::Kwargs(kwargs) => (vec![], kwargs.into_py_objects(heap, interns)),
            Self::ArgsKargs { args, kwargs } => (
                args.into_iter().map(|v| MontyObject::new(v, heap, interns)).collect(),
                kwargs.into_py_objects(heap, interns),
            ),
        }
    }

    /// Returns the number of positional arguments.
    ///
    /// For `Kwargs` returns 0, for `ArgsKargs` returns only the positional args count.
    fn count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Kwargs(_) => 0,
            Self::ArgsKargs { args, .. } => args.len(),
        }
    }
}

impl<T: ResourceTracker> DropWithHeap<T> for ArgValues {
    fn drop_with_heap(self, heap: &mut Heap<T>) {
        match self {
            Self::Empty => {}
            Self::One(v) => v.drop_with_heap(heap),
            Self::Two(v1, v2) => {
                v1.drop_with_heap(heap);
                v2.drop_with_heap(heap);
            }
            Self::Kwargs(kwargs) => {
                kwargs.drop_with_heap(heap);
            }
            Self::ArgsKargs { args, kwargs } => {
                args.drop_with_heap(heap);
                kwargs.drop_with_heap(heap);
            }
        }
    }
}

/// Iterator over positional arguments without allocation.
///
/// Supports iterating over `ArgValues::One/Two` without converting to Vec.
/// This iterator must be fully consumed OR explicitly dropped with
/// `drop_remaining_with_heap()` to maintain correct reference counts.
///
/// The iterator yields values by ownership transfer. Once a value is yielded,
/// the caller is responsible for either using it or calling `drop_with_heap()` on it.
pub(crate) enum ArgPosIter {
    Empty,
    One(Value),
    Two([Value; 2]),
    Vec(IntoIter<Value>),
}

impl ArgPosIter {
    /// Returns a slice of the remaining positional arguments without consuming them.
    pub fn as_slice(&self) -> &[Value] {
        match self {
            Self::Empty => &[],
            Self::One(v) => std::slice::from_ref(v),
            Self::Two(array) => array.as_slice(),
            Self::Vec(iter) => iter.as_slice(),
        }
    }
}

impl Iterator for ArgPosIter {
    type Item = Value;

    #[inline]
    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::One(_) => {
                let Self::One(v) = std::mem::replace(self, Self::Empty) else {
                    unreachable!()
                };
                Some(v)
            }
            Self::Two(_) => {
                let Self::Two([v1, v2]) = std::mem::replace(self, Self::Empty) else {
                    unreachable!()
                };
                *self = Self::One(v2);
                Some(v1)
            }
            Self::Vec(iter) => iter.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Empty => (0, Some(0)),
            Self::One(_) => (1, Some(1)),
            Self::Two(_) => (2, Some(2)),
            Self::Vec(iter) => iter.size_hint(),
        }
    }
}

impl ExactSizeIterator for ArgPosIter {}

impl<T: ResourceTracker> DropWithHeap<T> for ArgPosIter {
    fn drop_with_heap(self, heap: &mut Heap<T>) {
        match self {
            Self::Empty => {}
            Self::One(v1) => v1.drop_with_heap(heap),
            Self::Two(v12) => v12.drop_with_heap(heap),
            Self::Vec(iter) => iter.drop_with_heap(heap),
        }
    }
}

/// Type for keyword arguments.
///
/// Used to capture both the case of inline keyword arguments `foo(foo=1, bar=2)`
/// and the case of a dictionary passed as a single argument `foo(**kwargs)`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum KwargsValues {
    Empty,
    Inline(Vec<(StringId, Value)>),
    Dict(Dict),
}

impl KwargsValues {
    /// Returns the number of keyword arguments.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Inline(kvs) => kvs.len(),
            Self::Dict(dict) => dict.len(),
        }
    }

    /// Returns true if there are no keyword arguments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Converts the arguments into a Vec of MontyObjects.
    ///
    /// This is used when passing arguments to external functions.
    fn into_py_objects(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Vec<(MontyObject, MontyObject)> {
        match self {
            Self::Empty => vec![],
            Self::Inline(kvs) => kvs
                .into_iter()
                .map(|(k, v)| {
                    let key = MontyObject::String(interns.get_str(k).to_owned());
                    let value = MontyObject::new(v, heap, interns);
                    (key, value)
                })
                .collect(),
            Self::Dict(dict) => dict
                .into_iter()
                .map(|(k, v)| (MontyObject::new(k, heap, interns), MontyObject::new(v, heap, interns)))
                .collect(),
        }
    }

    /// Helper for functions which do not yet support kwargs, returns an `Err` if there are kwargs.
    pub fn not_supported_yet(self, method_name: &str, heap: &mut Heap<impl ResourceTracker>) -> RunResult<()> {
        if self.is_empty() {
            Ok(())
        } else {
            self.drop_with_heap(heap);
            Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!("{method_name}() does not support keyword arguments yet"),
            )
            .into())
        }
    }

    /// Properly drops all values in the arguments, decrementing reference counts.
    pub fn drop_with_heap(self, heap: &mut Heap<impl ResourceTracker>) {
        match self {
            Self::Empty => {}
            Self::Inline(kvs) => {
                for (_, v) in kvs {
                    v.drop_with_heap(heap);
                }
            }
            Self::Dict(dict) => {
                for (k, v) in dict {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
            }
        }
    }
}

impl IntoIterator for KwargsValues {
    type Item = (Value, Value);
    type IntoIter = KwargsValuesIter;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Empty => KwargsValuesIter::Empty,
            Self::Inline(kvs) => KwargsValuesIter::Inline(kvs.into_iter()),
            Self::Dict(dict) => KwargsValuesIter::Dict(dict.into_iter()),
        }
    }
}

/// Iterator over keyword argument (key, value) pairs.
///
/// For `Inline` kwargs, converts `StringId` keys to `Value::InternString`.
/// For `Dict` kwargs, iterates directly over the dict's entries without
/// intermediate allocation.
pub(crate) enum KwargsValuesIter {
    Empty,
    Inline(IntoIter<(StringId, Value)>),
    Dict(DictIntoIter),
}

impl Iterator for KwargsValuesIter {
    type Item = (Value, Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Inline(iter) => iter.next().map(|(k, v)| (Value::InternString(k), v)),
            Self::Dict(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Empty => (0, Some(0)),
            Self::Inline(iter) => iter.size_hint(),
            Self::Dict(iter) => iter.size_hint(),
        }
    }
}

impl ExactSizeIterator for KwargsValuesIter {}

impl<T: ResourceTracker> DropWithHeap<T> for KwargsValuesIter {
    fn drop_with_heap(self, heap: &mut Heap<T>) {
        match self {
            Self::Empty => {}
            Self::Inline(iter) => {
                for (_, v) in iter {
                    v.drop_with_heap(heap);
                }
            }
            Self::Dict(iter) => {
                for (k, v) in iter {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
            }
        }
    }
}

/// A keyword argument in a function call expression.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Kwarg {
    pub key: Identifier,
    pub value: ExprLoc,
}

/// Expressions that make up a function call's arguments.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ArgExprs {
    Empty,
    One(ExprLoc),
    Two(ExprLoc, ExprLoc),
    Args(Vec<ExprLoc>),
    Kwargs(Vec<Kwarg>),
    ArgsKargs {
        args: Option<Vec<ExprLoc>>,
        var_args: Option<ExprLoc>,
        kwargs: Option<Vec<Kwarg>>,
        var_kwargs: Option<ExprLoc>,
    },
}

impl ArgExprs {
    /// Creates a new `ArgExprs` with optional `*args` and `**kwargs` unpacking expressions.
    ///
    /// This is used when parsing function calls that may include `*expr` / `**expr`
    /// syntax for unpacking iterables or mappings into arguments.
    pub fn new_with_var_kwargs(
        args: Vec<ExprLoc>,
        var_args: Option<ExprLoc>,
        kwargs: Vec<Kwarg>,
        var_kwargs: Option<ExprLoc>,
    ) -> Self {
        // Full generality requires ArgsKargs when we have unpacking or mixed arg/kwarg usage
        if var_args.is_some() || var_kwargs.is_some() || (!kwargs.is_empty() && !args.is_empty()) {
            Self::ArgsKargs {
                args: if args.is_empty() { None } else { Some(args) },
                var_args,
                kwargs: if kwargs.is_empty() { None } else { Some(kwargs) },
                var_kwargs,
            }
        } else if !kwargs.is_empty() {
            Self::Kwargs(kwargs)
        } else if args.len() > 2 {
            Self::Args(args)
        } else {
            let mut iter = args.into_iter();
            if let Some(first) = iter.next() {
                if let Some(second) = iter.next() {
                    Self::Two(first, second)
                } else {
                    Self::One(first)
                }
            } else {
                Self::Empty
            }
        }
    }

    /// Applies a transformation function to all `ExprLoc` elements in the args.
    ///
    /// This is used during the preparation phase to recursively prepare all
    /// argument expressions before execution.
    pub fn prepare_args(
        &mut self,
        mut f: impl FnMut(ExprLoc) -> Result<ExprLoc, ParseError>,
    ) -> Result<(), ParseError> {
        // Swap self with Empty to take ownership, then rebuild
        let taken = std::mem::replace(self, Self::Empty);
        *self = match taken {
            Self::Empty => Self::Empty,
            Self::One(arg) => Self::One(f(arg)?),
            Self::Two(arg1, arg2) => Self::Two(f(arg1)?, f(arg2)?),
            Self::Args(args) => Self::Args(args.into_iter().map(&mut f).collect::<Result<Vec<_>, _>>()?),
            Self::Kwargs(kwargs) => Self::Kwargs(
                kwargs
                    .into_iter()
                    .map(|kwarg| {
                        Ok(Kwarg {
                            key: kwarg.key,
                            value: f(kwarg.value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, ParseError>>()?,
            ),
            Self::ArgsKargs {
                args,
                var_args,
                kwargs,
                var_kwargs,
            } => {
                let args = args
                    .map(|a| a.into_iter().map(&mut f).collect::<Result<Vec<_>, ParseError>>())
                    .transpose()?;
                let var_args = var_args.map(&mut f).transpose()?;
                let kwargs = kwargs
                    .map(|k| {
                        k.into_iter()
                            .map(|kwarg| {
                                Ok(Kwarg {
                                    key: kwarg.key,
                                    value: f(kwarg.value)?,
                                })
                            })
                            .collect::<Result<Vec<_>, ParseError>>()
                    })
                    .transpose()?;
                let var_kwargs = var_kwargs.map(&mut f).transpose()?;
                Self::ArgsKargs {
                    args,
                    var_args,
                    kwargs,
                    var_kwargs,
                }
            }
        };
        Ok(())
    }
}
