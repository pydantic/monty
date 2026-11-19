//! Implementation of the `annotationlib` module.
//!
//! - `Format`: the `VALUE` / `VALUE_WITH_FAKE_GLOBALS` / `FORWARDREF` / `STRING`
//!   constants, as plain ints on a class (Monty has no `enum` module)
//! - `ForwardRef`: an annotation kept as source text (see `types/forward_ref.rs`)
//! - `get_annotations()`: an object's annotations evaluated as written, which
//!   `typing.get_type_hints()` builds on

use monty_macros::py_signature;

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    intern::StaticStrings,
    modules::{
        ModuleFunctions,
        typing::{HintFormat, HintsRequest, HintsTask, collect_hints, namespace_dict},
    },
    types::{Class, Dict, Module, PyTrait, Type, forward_ref::format_code},
    value::{EitherStr, Value},
};

/// Creates the `annotationlib` module.
pub fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Annotationlib, vm.interns);
    let format = format_class(vm);
    module.set_attr(StaticStrings::FormatClass, format, vm);
    module.set_attr(
        StaticStrings::ForwardRef,
        Value::Builtin(Builtins::Type(Type::ForwardRef)),
        vm,
    );
    module.set_attr(
        StaticStrings::GetAnnotations,
        Value::ModuleFunction(ModuleFunctions::Annotationlib(AnnotationlibFunctions::GetAnnotations)),
        vm,
    );
    module
}

/// `annotationlib` module functions, each a Python-visible callable.
#[monty_macros::explicit_discriminants(checksum = 0xec85_305b_4380_4f1f)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum AnnotationlibFunctions {
    GetAnnotations = 0,
}

/// Dispatches an `annotationlib` module function call.
pub(super) fn call(vm: &mut VM<'_>, function: AnnotationlibFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        AnnotationlibFunctions::GetAnnotations => get_annotations(vm, args),
    }
}

/// The `Format` class: `VALUE = 1`, `VALUE_WITH_FAKE_GLOBALS = 2`, `FORWARDREF = 3`,
/// `STRING = 4` as plain ints, so `format=Format.STRING` and `format=4` agree.
fn format_class(vm: &mut VM<'_>) -> Value {
    let members = [
        (StaticStrings::ValueFormat, Value::Int(1)),
        (StaticStrings::ValueWithFakeGlobals, Value::Int(2)),
        (StaticStrings::Forwardref, Value::Int(3)),
        (StaticStrings::StringFormat, Value::Int(4)),
        (StaticStrings::DunderDoc, Value::None),
    ];
    let mut namespace = Dict::new();
    for (name, value) in members {
        let key = Value::InternString(vm.interns.intern_static(name));
        // Module construction is infallible: interned keys always hash, and a
        // handful of entries needs no growth preflight.
        namespace
            .set_without_growth_check(key, value, vm)
            .expect("class attribute keys are interned, so hashing cannot fail");
    }
    let name = EitherStr::Interned(vm.interns.intern_static(StaticStrings::FormatClass));
    Value::Ref(vm.heap.allocate(HeapData::Class(Box::new(Class::new(name, namespace)))))
}

/// `annotationlib.get_annotations()`: `obj`'s annotations evaluated where they were
/// written (or as stored, in `STRING` format), without resolving forward references.
#[py_signature("get_annotations(obj, *, globals=None, locals=None, eval_str=False, format=1)")]
fn get_annotations(vm: &mut VM<'_>, args: &GetAnnotationsArgs) -> RunResult<CallResult> {
    let eval_str = args.eval_str.py_bool(vm)?;
    if eval_str && format_code(&args.format) != Some(1) {
        return Err(ExcType::value_error(
            "eval_str=True is only supported with format=Format.VALUE",
        ));
    }
    let format = HintFormat::parse(&args.format, false, vm)?;
    let globalns = namespace_dict(&args.globals, "get_annotations", "globals", vm)?;
    let localns = namespace_dict(&args.locals, "get_annotations", "locals", vm)?;
    collect_hints(
        &args.obj,
        &HintsRequest {
            format,
            globalns,
            localns,
            include_extras: true,
            task: HintsTask::Annotations { eval_str },
        },
        vm,
    )
}
