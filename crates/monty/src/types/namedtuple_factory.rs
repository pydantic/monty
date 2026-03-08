//! Heap-backed callable factory for `collections.namedtuple()`.
//!
//! CPython's `collections.namedtuple()` returns a new class. Monty does not yet
//! implement Python class definitions, so this type provides the smallest clean
//! runtime abstraction that still behaves like a constructor: it captures the
//! typename, validated field names, defaults, and optional module metadata, and
//! when called it produces the existing [`NamedTuple`](crate::types::NamedTuple)
//! runtime value.

use std::fmt::Write;

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult},
    heap::{ContainsHeap, DropWithHeap, Heap, HeapData, HeapGuard, HeapId},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
    types::{
        MontyIter, NamedTuple, PyTrait, Type,
        re_pattern::value_to_str,
        str::{is_python_identifier, is_python_keyword},
    },
    value::{EitherStr, Value},
};

/// Callable factory returned by `collections.namedtuple()`.
///
/// The factory is immutable after creation and can be serialized inside REPL
/// snapshots. It keeps only the metadata needed for constructor-like calls and
/// class-style repr output.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamedTupleFactory {
    /// Bare typename used by instance repr, e.g. `Point`.
    typename: EitherStr,
    /// Validated field names in constructor order.
    field_names: Vec<EitherStr>,
    /// Right-aligned trailing defaults for omitted constructor arguments.
    defaults: Vec<Value>,
    /// Optional module metadata used only for the class-like repr.
    module_name: Option<EitherStr>,
    /// Cached reference presence for GC/refcount optimizations.
    contains_refs: bool,
}

impl NamedTupleFactory {
    /// Creates a new namedtuple factory from validated components.
    #[must_use]
    pub fn new(
        typename: EitherStr,
        field_names: Vec<EitherStr>,
        defaults: Vec<Value>,
        module_name: Option<EitherStr>,
    ) -> Self {
        let contains_refs = defaults.iter().any(|value| matches!(value, Value::Ref(_)));
        Self {
            typename,
            field_names,
            defaults,
            module_name,
            contains_refs,
        }
    }

    /// Builds and allocates a namedtuple factory from a `collections.namedtuple()` call.
    ///
    /// This implements the public API supported by Monty:
    /// `namedtuple(typename, field_names, *, rename=False, module=None, defaults=None)`.
    pub fn create_from_namedtuple_call(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let factory = parse_namedtuple_args(vm, args)?;
        let heap_id = vm.heap.allocate(HeapData::NamedTupleFactory(factory))?;
        Ok(Value::Ref(heap_id))
    }

    /// Calls the factory like a class constructor, returning a `NamedTuple`.
    ///
    /// Constructor binding matches Python's positional/keyword rules closely
    /// enough for ordinary namedtuple usage, including defaults and exact error
    /// messages for the covered cases.
    pub fn call(&self, args: ArgValues, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Value> {
        let constructor_name = self.constructor_name(interns);
        let field_count = self.field_names.len();

        let (positional, kwargs) = args.into_parts();
        let positional_count = positional.len();
        defer_drop_mut!(positional, heap);
        let kwargs_count = kwargs.len();
        let kwargs_iter = kwargs.into_iter();
        defer_drop_mut!(kwargs_iter, heap);

        if positional_count > field_count {
            return Err(ExcType::type_error_too_many_positional(
                &constructor_name,
                field_count + 1,
                positional_count + 1,
                kwargs_count,
            ));
        }

        let mut assigned_guard = HeapGuard::new((0..field_count).map(|_| None).collect::<Vec<Option<Value>>>(), heap);
        {
            let (assigned, heap) = assigned_guard.as_parts_mut();

            for (index, value) in positional.enumerate() {
                assigned[index] = Some(value);
            }

            for (key, value) in kwargs_iter {
                defer_drop!(key, heap);
                let mut value_guard = HeapGuard::new(value, heap);
                let Some(keyword_name) = key.as_either_str(value_guard.heap().heap()) else {
                    return Err(ExcType::type_error_kwargs_nonstring_key());
                };
                let keyword = keyword_name.as_str(interns);
                let Some(index) = self.field_index(keyword, interns) else {
                    return Err(ExcType::type_error_unexpected_keyword(&constructor_name, keyword));
                };
                if assigned[index].is_some() {
                    return Err(ExcType::type_error_duplicate_arg(&constructor_name, keyword));
                }
                assigned[index] = Some(value_guard.into_inner());
            }

            let required_count = field_count.saturating_sub(self.defaults.len());
            for (default, value) in self.defaults.iter().zip(assigned.iter_mut().skip(required_count)) {
                if value.is_none() {
                    *value = Some(default.clone_with_heap(heap));
                }
            }
        }

        let required_count = field_count.saturating_sub(self.defaults.len());
        let assigned = &assigned_guard.as_parts().0;
        let missing_fields: Vec<&str> = assigned
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                if index < required_count && value.is_none() {
                    Some(self.field_names[index].as_str(interns))
                } else {
                    None
                }
            })
            .collect();
        if !missing_fields.is_empty() {
            return Err(ExcType::type_error_missing_positional_with_names(
                &constructor_name,
                &missing_fields,
            ));
        }

        let values: Vec<Value> = assigned_guard
            .into_inner()
            .into_iter()
            .map(|value| value.expect("namedtuple constructor left value unbound"))
            .collect();
        let namedtuple = NamedTuple::new(self.typename.clone(), self.field_names.clone(), values);
        Ok(Value::Ref(heap.allocate(HeapData::NamedTuple(namedtuple))?))
    }

    /// Returns the constructor name used in error messages.
    fn constructor_name(&self, interns: &Interns) -> String {
        format!("{}.__new__", self.typename.as_str(interns))
    }

    /// Returns the field index for a keyword binding.
    fn field_index(&self, field_name: &str, interns: &Interns) -> Option<usize> {
        self.field_names
            .iter()
            .position(|candidate| candidate.as_str(interns) == field_name)
    }

    /// Returns the fully qualified display name used by the class-like repr.
    fn qualified_name(&self, interns: &Interns) -> String {
        let typename = self.typename.as_str(interns);
        match &self.module_name {
            Some(module_name) if !module_name.as_str(interns).is_empty() => {
                format!("{}.{}", module_name.as_str(interns), typename)
            }
            _ => typename.to_owned(),
        }
    }

    /// Returns whether the factory currently holds any heap references in defaults.
    #[must_use]
    pub fn contains_refs(&self) -> bool {
        self.contains_refs
    }

    /// Returns the default values stored on the factory.
    #[must_use]
    pub fn defaults(&self) -> &[Value] {
        &self.defaults
    }
}

impl PyTrait for NamedTupleFactory {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Type
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(
        &self,
        _other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        Ok(false)
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.typename.py_estimate_size()
            + self.field_names.iter().map(EitherStr::py_estimate_size).sum::<usize>()
            + self.defaults.iter().map(Value::py_estimate_size).sum::<usize>()
            + self.module_name.as_ref().map_or(0, EitherStr::py_estimate_size)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if !self.contains_refs {
            return;
        }
        for default in &mut self.defaults {
            default.py_dec_ref_ids(stack);
        }
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        write!(f, "<class '{}'>", self.qualified_name(interns))
    }
}

/// Parses and validates a `collections.namedtuple()` call.
fn parse_namedtuple_args(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<NamedTupleFactory> {
    let (positional, kwargs) = args.into_parts();
    let positional_count = positional.len();
    defer_drop_mut!(positional, vm);
    let kwargs_count = kwargs.len();
    let kwargs_iter = kwargs.into_iter();
    defer_drop_mut!(kwargs_iter, vm);

    if positional_count > 2 {
        return Err(ExcType::type_error_too_many_positional(
            "namedtuple",
            2,
            positional_count,
            kwargs_count,
        ));
    }

    let mut values_guard = HeapGuard::new((0..5).map(|_| None).collect::<Vec<Option<Value>>>(), vm);
    {
        let (values, vm) = values_guard.as_parts_mut();
        let interns = vm.interns;
        for (index, value) in positional.enumerate() {
            values[index] = Some(value);
        }

        for (key, value) in kwargs_iter {
            defer_drop!(key, vm);
            let mut value_guard = HeapGuard::new(value, vm);
            let Some(keyword_name) = key.as_either_str(value_guard.heap().heap()) else {
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            let keyword = keyword_name.as_str(interns);
            let Some(index) = namedtuple_param_index(keyword) else {
                return Err(ExcType::type_error_unexpected_keyword("namedtuple", keyword));
            };
            if values[index].is_some() {
                return Err(ExcType::type_error_duplicate_arg("namedtuple", keyword));
            }
            values[index] = Some(value_guard.into_inner());
        }
    }

    let (values, _) = values_guard.as_parts();
    let missing_names: Vec<&str> = values[..2]
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if value.is_none() {
                Some(if index == 0 { "typename" } else { "field_names" })
            } else {
                None
            }
        })
        .collect();
    if !missing_names.is_empty() {
        return Err(ExcType::type_error_missing_positional_with_names(
            "namedtuple",
            &missing_names,
        ));
    }

    let mut values = values_guard.into_inner();
    let typename_value = values[0].take().expect("typename should be bound");
    let field_names_value = values[1].take().expect("field_names should be bound");
    let rename_value = values[2].take();
    let module_value = values[3].take();
    let defaults_value = values[4].take();
    values.drop_with_heap(vm);

    let typename = parse_typename(typename_value, vm.heap, vm.interns)?;
    let rename = parse_rename(rename_value, vm.heap, vm.interns);
    let raw_field_names = parse_field_names(vm, field_names_value)?;
    let field_names = validate_field_names(raw_field_names, rename)?;
    let module_name = parse_module_name(module_value, vm.heap, vm.interns)?;
    let defaults = parse_defaults(vm, defaults_value)?;

    if defaults.len() > field_names.len() {
        defaults.drop_with_heap(vm);
        return Err(ExcType::namedtuple_too_many_defaults());
    }

    Ok(NamedTupleFactory::new(
        typename.into(),
        field_names.into_iter().map(Into::into).collect(),
        defaults,
        module_name.map(Into::into),
    ))
}

/// Maps namedtuple parameter names to storage indexes in the temporary binding vector.
fn namedtuple_param_index(name: &str) -> Option<usize> {
    match name {
        "typename" => Some(0),
        "field_names" => Some(1),
        "rename" => Some(2),
        "module" => Some(3),
        "defaults" => Some(4),
        _ => None,
    }
}

/// Parses the typename argument.
fn parse_typename(value: Value, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<String> {
    defer_drop!(value, heap);
    let typename = value.py_str(heap, interns).into_owned();
    validate_typename(&typename)?;
    Ok(typename)
}

/// Parses the rename keyword argument.
fn parse_rename(value: Option<Value>, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
    value.is_some_and(|value| {
        let result = value.py_bool(heap, interns);
        value.drop_with_heap(heap);
        result
    })
}

/// Parses the field_names argument, accepting either a string or an iterable of values.
fn parse_field_names(vm: &mut VM<'_, '_, impl ResourceTracker>, value: Value) -> RunResult<Vec<String>> {
    if let Some(field_names_str) = value.as_either_str(vm.heap) {
        let field_names = split_field_names(field_names_str.as_str(vm.interns))
            .into_iter()
            .map(str::to_owned)
            .collect();
        value.drop_with_heap(vm);
        return Ok(field_names);
    }

    let iter = MontyIter::new(value, vm)?;
    let items: Vec<Value> = iter.collect(vm)?;
    let mut field_names = Vec::with_capacity(items.len());
    for item in items {
        let field_name = item.py_str(vm.heap, vm.interns).into_owned();
        item.drop_with_heap(vm);
        field_names.push(field_name);
    }
    Ok(field_names)
}

/// Parses the module keyword argument.
fn parse_module_name(
    value: Option<Value>,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    defer_drop!(value, heap);
    if matches!(value, Value::None) {
        return Ok(None);
    }
    Ok(Some(value_to_str(value, heap, interns)?.into_owned()))
}

/// Parses the defaults keyword argument.
fn parse_defaults(vm: &mut VM<'_, '_, impl ResourceTracker>, value: Option<Value>) -> RunResult<Vec<Value>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if matches!(value, Value::None) {
        value.drop_with_heap(vm);
        return Ok(Vec::new());
    }
    let iter = MontyIter::new(value, vm)?;
    iter.collect(vm)
}

/// Validates the typename against Python identifier rules.
fn validate_typename(typename: &str) -> RunResult<()> {
    if !is_python_identifier(typename) {
        return Err(ExcType::namedtuple_invalid_identifier(typename));
    }
    if is_python_keyword(typename) {
        return Err(ExcType::namedtuple_keyword(typename));
    }
    Ok(())
}

/// Validates and possibly renames field names.
fn validate_field_names(field_names: Vec<String>, rename: bool) -> RunResult<Vec<String>> {
    let mut validated = Vec::with_capacity(field_names.len());

    for (index, field_name) in field_names.into_iter().enumerate() {
        let needs_rename = !is_python_identifier(&field_name)
            || is_python_keyword(&field_name)
            || field_name.starts_with('_')
            || validated.iter().any(|existing| existing == &field_name);

        if rename && needs_rename {
            validated.push(format!("_{index}"));
            continue;
        }

        if !is_python_identifier(&field_name) {
            return Err(ExcType::namedtuple_invalid_identifier(&field_name));
        }
        if is_python_keyword(&field_name) {
            return Err(ExcType::namedtuple_keyword(&field_name));
        }
        if field_name.starts_with('_') {
            return Err(ExcType::namedtuple_field_starts_with_underscore(&field_name));
        }
        if validated.iter().any(|existing| existing == &field_name) {
            return Err(ExcType::namedtuple_duplicate_field(&field_name));
        }

        validated.push(field_name);
    }

    Ok(validated)
}

/// Splits a field-name string on commas and ASCII whitespace like CPython.
fn split_field_names(field_names: &str) -> Vec<&str> {
    field_names
        .split(|char: char| char == ',' || char.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .collect()
}
