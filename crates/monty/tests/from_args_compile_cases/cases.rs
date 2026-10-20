//! Accepted generic argument declarations, compiled inside the interpreter by the UI harness.

use std::marker::PhantomData;

use crate::{
    args::{ArgValues, FromArgs, FromValue, FromValueFail},
    bytecode::VM,
    exception_private::RunResult,
    heap::{ContainsHeap, RuntimeRef},
    value::Value,
};

/// Models an owned extraction result carrying an invariant activation lifetime.
struct Scoped<'scope, H, T>(T, PhantomData<(fn(&'scope ()) -> &'scope (), H)>);

impl<H: RuntimeRef, T: FromValue<H>> FromValue<H> for Scoped<'_, H, T> {
    fn from_value(value: Value<H>, vm: &mut VM<'_, H>) -> Result<Self, FromValueFail> {
        T::from_value(value, vm).map(|value| Self(value, PhantomData))
    }

    fn drop_extracted(self, heap: &mut impl ContainsHeap<Ref = H>) {
        self.0.drop_extracted(heap);
    }
}

/// Covers lifetimes, defaulted type/const parameters, where clauses and generated-name collisions.
#[derive(FromArgs)]
#[from_args(name = "generic")]
struct Generic<'scope, H: RuntimeRef, __C = Value<H>, const N: usize = 3>
where
    __C: FromValue<H>,
{
    #[from_args(pos_only)]
    value: Scoped<'scope, H, __C>,
    #[from_args(pos_only, default)]
    __from_args_marker: bool,
}

/// A monomorphized use also checks inference of the cleanup slots' const parameter.
#[expect(
    dead_code,
    reason = "compile-pass control; the UI harness checks this body without executing it"
)]
fn extract<H: RuntimeRef>(args: ArgValues<H>, vm: &mut VM<'_, H>) -> RunResult<()> {
    let result = Generic::<'_, H, Value<H>, 7>::from_args(args, vm)?;
    result.value.drop_extracted(vm);
    Ok(())
}
