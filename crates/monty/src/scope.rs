//! Temporaries owned by one native call or loop iteration.

use std::{
    marker::PhantomData,
    mem::{self, ManuallyDrop},
    ops::Deref,
};

use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop_mut,
    exception_private::RunResult,
    value::Value,
};

pub(crate) struct ScopedVM<'s, 'h> {
    values: &'s mut SmallVec<[Value; 3]>,
    vm: &'s mut VM<'h>,
}

/// The brand ties ownership transfers to the scope that admitted the value.
pub(crate) struct Local<'s> {
    repr: LocalRepr,
    brand: PhantomData<fn(&'s ()) -> &'s ()>,
}

enum LocalRepr {
    Immediate(Value),
    /// The scope owns the reference count. This view only contains its heap ID.
    Heap {
        view: ManuallyDrop<Value>,
        slot: usize,
    },
}

impl Deref for Local<'_> {
    type Target = Value;

    fn deref(&self) -> &Value {
        match &self.repr {
            LocalRepr::Immediate(value) => value,
            LocalRepr::Heap { view, .. } => view,
        }
    }
}

impl<'h> VM<'h> {
    /// Borrows the bound arguments for one call and drops their heap references on return.
    pub(crate) fn with_args<A: FromArgs, R>(
        &mut self,
        args: ArgValues,
        run: impl FnOnce(&A, &mut Self) -> RunResult<R>,
    ) -> RunResult<R> {
        self.with_args_mut(args, |bound, vm| run(bound, vm))
    }

    pub(crate) fn with_args_mut<A: FromArgs, R>(
        &mut self,
        args: ArgValues,
        run: impl FnOnce(&mut A, &mut Self) -> RunResult<R>,
    ) -> RunResult<R> {
        let vm = self;
        let bound = A::from_args(args, vm)?;
        defer_drop_mut!(bound, vm);
        run(bound, vm)
    }

    /// Releases the retained values when the callback exits.
    pub(crate) fn scope<R>(&mut self, run: impl for<'s> FnOnce(&mut ScopedVM<'s, 'h>) -> R) -> R {
        let values = SmallVec::new();
        let vm = self;
        defer_drop_mut!(values, vm);
        run(&mut ScopedVM { values, vm })
    }
}

#[expect(clippy::inline_always, reason = "inlining keep and take reduces dict.update latency")]
impl<'s, 'h> ScopedVM<'s, 'h> {
    /// Transfers a value into the scope and returns a handle for borrowing it.
    #[inline(always)]
    pub(crate) fn keep(&mut self, value: Value) -> Local<'s> {
        let repr = if let Value::Ref(id) = value {
            let slot = self.values.len();
            self.values.push(value);
            LocalRepr::Heap {
                view: ManuallyDrop::new(Value::Ref(id)),
                slot,
            }
        } else {
            LocalRepr::Immediate(value)
        };
        Local {
            repr,
            brand: PhantomData,
        }
    }

    /// Transfers the value out so it can be returned or stored.
    #[inline(always)]
    pub(crate) fn take(&mut self, handle: Local<'s>) -> Value {
        match handle.repr {
            LocalRepr::Immediate(value) => value,
            LocalRepr::Heap { slot, .. } => mem::replace(&mut self.values[slot], Value::None),
        }
    }

    pub(crate) fn vm(&mut self) -> &mut VM<'h> {
        self.vm
    }
}

#[cfg(all(test, feature = "ref-count-return"))]
mod tests {
    use monty_types::{PrintWriter, ResourceTracker};

    use super::*;
    use crate::{
        args::{KwargsValues, StrArg},
        defer_drop,
        exception_private::{ExcType, ExcTypeExt},
        heap::{Heap, HeapReader},
        run::{Program, SessionTables},
        types::str::allocate_string_on_heap,
    };

    fn with_vm(run: impl FnOnce(&mut VM<'_>)) {
        let mut heap = Heap::new(16, ResourceTracker::default());
        let mut tables = SessionTables::default();
        let program = Program::for_tests();
        HeapReader::with(&mut heap, &mut (&program, &mut tables), |heap, (program, tables)| {
            run(&mut VM::new(Vec::new(), tables, program, heap, PrintWriter::Disabled));
        });
    }

    #[derive(FromArgs)]
    #[from_args(name = "collect")]
    struct CollectArgs {
        #[from_args(pos_only)]
        text: StrArg,
        #[from_args(varargs)]
        rest: Vec<Value>,
        #[from_args(varkwargs)]
        keywords: KwargsValues,
    }

    #[test]
    fn single_positional_conversion_releases_rejected_value() {
        #[derive(FromArgs)]
        #[from_args(name = "count")]
        struct CountArgs {
            #[from_args(pos_only)]
            count: i64,
            #[from_args(varkwargs)]
            keywords: KwargsValues,
        }

        with_vm(|vm| {
            let value = allocate_string_on_heap("not an integer", vm.heap);
            defer_drop!(value, vm);
            let Value::Ref(id) = *value else { unreachable!() };
            let args = ArgValues::One(value.clone_with_heap(vm));
            assert!(CountArgs::from_args(args, vm).is_err());
            assert_eq!(vm.heap.get_refcount(id), 1);
        });
    }

    #[test]
    fn conversion_error_releases_forwarded_keywords() {
        with_vm(|vm| {
            let value = allocate_string_on_heap("keyword value", vm.heap);
            defer_drop!(value, vm);
            let Value::Ref(id) = *value else { unreachable!() };
            let key = vm.interns.intern_runtime("text");
            let args = ArgValues::ArgsKargs {
                args: vec![Value::None],
                kwargs: KwargsValues::Inline(vec![(key, value.clone_with_heap(vm))]),
            };
            assert!(CollectArgs::from_args(args, vm).is_err());
            assert_eq!(vm.heap.get_refcount(id), 1);
        });
    }

    #[test]
    fn with_args_releases_typed_fields_and_collectors_on_error() {
        with_vm(|vm| {
            let value = allocate_string_on_heap("key", vm.heap);
            defer_drop!(value, vm);
            let Value::Ref(id) = *value else { unreachable!() };
            let clone = || value.clone_with_heap(vm.heap);
            let args = ArgValues::ArgsKargs {
                args: vec![clone(), clone()],
                kwargs: KwargsValues::Pairs(vec![(clone(), clone())]),
            };
            let result = vm.with_args(args, |args: &CollectArgs, vm| {
                assert_eq!(args.text.as_str(vm), "key");
                assert_eq!(args.rest.len(), 1);
                assert_eq!(args.keywords.len(), 1);
                assert_eq!(vm.heap.get_refcount(id), 5);
                Err::<(), _>(ExcType::type_error("callback failed"))
            });
            assert!(result.is_err());
            assert_eq!(vm.heap.get_refcount(id), 1);
        });
    }

    #[test]
    fn nested_scope_releases_on_error_and_take_transfers_ownership() {
        with_vm(|vm| {
            let value = allocate_string_on_heap("test", vm.heap);
            defer_drop!(value, vm);
            let Value::Ref(id) = *value else { unreachable!() };

            let taken = vm.scope(|scope| {
                let immediate = scope.keep(Value::Int(7));
                assert!(scope.values.is_empty());
                assert!(matches!(scope.take(immediate), Value::Int(7)));

                let cloned = value.clone_with_heap(scope.vm().heap);
                let held = scope.keep(cloned);
                let error = scope.vm().scope(|scope| {
                    let cloned = held.clone_with_heap(scope.vm().heap);
                    let _inner = scope.keep(cloned);
                    assert_eq!(scope.vm().heap.get_refcount(id), 3);
                    Err::<(), ()>(())
                });
                assert!(error.is_err());
                assert_eq!(scope.vm().heap.get_refcount(id), 2);

                let cloned = held.clone_with_heap(scope.vm().heap);
                let returned = scope.keep(cloned);
                scope.take(returned)
            });

            assert_eq!(vm.heap.get_refcount(id), 2);
            taken.drop_with(vm);
            assert_eq!(vm.heap.get_refcount(id), 1);
        });
    }
}
