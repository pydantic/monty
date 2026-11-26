use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::HeapData,
    resource_checks::try_vec_with_capacity,
    types::{FrozenSet, Set, bytes::allocate_bytes},
    value::Value,
};

/// Names used by old protocols to represent built-in data.
#[derive(Clone, Copy)]
pub(super) enum Constructor {
    Bytes,
    Latin1,
    Set,
    FrozenSet,
}

impl Constructor {
    pub(super) fn from_name(module: &[u8], name: &[u8]) -> RunResult<Self> {
        match (module, name) {
            (b"__builtin__" | b"builtins", b"bytes") => Ok(Self::Bytes),
            (b"_codecs", b"encode") => Ok(Self::Latin1),
            (b"__builtin__" | b"builtins", b"set") => Ok(Self::Set),
            (b"__builtin__" | b"builtins", b"frozenset") => Ok(Self::FrozenSet),
            _ => Err(ExcType::value_error("unsupported pickle global")),
        }
    }

    pub(super) fn name(self, protocol: u8) -> &'static [u8] {
        match self {
            Self::Bytes => b"__builtin__\nbytes\n",
            Self::Latin1 => b"_codecs\nencode\n",
            Self::Set if protocol == 2 => b"__builtin__\nset\n",
            Self::Set => b"builtins\nset\n",
            Self::FrozenSet if protocol == 2 => b"__builtin__\nfrozenset\n",
            Self::FrozenSet => b"builtins\nfrozenset\n",
        }
    }

    /// Interprets the data encodings directly, without looking up or calling a global.
    pub(super) fn construct(self, args: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
        let Some(HeapData::Tuple(args)) = args.ref_id().map(|id| vm.heap.get(id)) else {
            return Err(invalid_args());
        };
        match (self, args.as_slice()) {
            (Self::Bytes, []) => Ok(allocate_bytes(Vec::new(), vm.heap)),
            (Self::Latin1, [text, codec]) if codec.as_str(vm) == Some("latin1") => {
                let text = text.as_str(vm).ok_or_else(invalid_args)?;
                vm.heap.tracker.check_allocation(text.len())?;
                let mut bytes = try_vec_with_capacity(text.len())?;
                for (i, ch) in text.chars().enumerate() {
                    bytes.push(u8::try_from(ch).map_err(|_| invalid_args())?);
                    vm.heap.tracker.check_time_every(i)?;
                }
                Ok(allocate_bytes(bytes, vm.heap))
            }
            (Self::Set | Self::FrozenSet, [items])
                if items
                    .ref_id()
                    .is_some_and(|id| matches!(vm.heap.get(id), HeapData::List(_) | HeapData::Tuple(_))) =>
            {
                let args = ArgValues::One(items.clone_with_heap(vm.heap));
                match self {
                    Self::Set => Set::init(vm, args),
                    _ => FrozenSet::init(vm, args),
                }
            }
            _ => Err(invalid_args()),
        }
    }
}

fn invalid_args() -> RunError {
    ExcType::value_error("invalid arguments for pickle data constructor")
}
