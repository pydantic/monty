use crate::{
    builtins::Builtins,
    heap::HeapObjectRead,
    py_value::PyValue,
    types::{Class, HostClassType, NamedTupleClass, Type},
};

/// A class object viewed through its existing native representation.
///
/// Heap variants borrow their payload without owning a Python reference.
pub(crate) enum PyClass<'h> {
    Builtin(Type),
    Custom(HeapObjectRead<'h, Class>),
    NamedTuple(HeapObjectRead<'h, NamedTupleClass>),
    Host(HeapObjectRead<'h, HostClassType>),
}

impl<'h> TryFrom<PyValue<'h>> for PyClass<'h> {
    type Error = PyValue<'h>;

    fn try_from(value: PyValue<'h>) -> Result<Self, Self::Error> {
        match value {
            PyValue::Builtin(Builtins::Type(class)) => Ok(Self::Builtin(class)),
            PyValue::Class(class) => Ok(Self::Custom(class)),
            PyValue::NamedTupleClass(class) => Ok(Self::NamedTuple(class)),
            PyValue::HostClassType(class) => Ok(Self::Host(class)),
            value => Err(value),
        }
    }
}

impl<'h> From<PyClass<'h>> for PyValue<'h> {
    fn from(class: PyClass<'h>) -> Self {
        match class {
            PyClass::Builtin(class) => Self::Builtin(Builtins::Type(class)),
            PyClass::Custom(class) => Self::Class(class),
            PyClass::NamedTuple(class) => Self::NamedTupleClass(class),
            PyClass::Host(class) => Self::HostClassType(class),
        }
    }
}
