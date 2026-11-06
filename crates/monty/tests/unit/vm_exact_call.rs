//! Private exact-call regression, included in the owning VM context.

use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

use super::VM;
use crate::{
    heap::{Heap, HeapReader},
    run::{Executor, Program},
    value::Value,
};

/// Exact heap-backed calls move owned arguments without allocating a binding namespace.
#[test]
fn heap_exact_call_keeps_arguments_on_stack() {
    let mut executor = Executor::new(
        "def pick(first, second, third):\n    return first\n\
         item = [1, 2, 3]\nalias = pick\n\
         assert alias(item, [], {}) is item\nlen(item)"
            .to_owned(),
        "exact.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let globals = (0..executor.namespace_size()).map(|_| Value::Undefined).collect();
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut executor, |reader, executor| {
        let mut vm = VM::new(
            globals,
            &mut executor.tables,
            &executor.program,
            reader,
            PrintWriter::Disabled,
        );
        assert_eq!(Program::run_to_completion(&mut vm).unwrap(), MontyObject::int(3));
        assert_eq!(vm.namespace_scratch.capacity(), 0);
    });
}
