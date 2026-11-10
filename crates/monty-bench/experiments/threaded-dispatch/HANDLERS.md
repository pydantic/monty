# Writing threaded opcode handlers

Declare each opcode once in `opcode_handlers!` in
[`dispatch_threaded.rs`](../../../monty/src/bytecode/vm/dispatch_threaded.rs).
The declaration generates its semantic handler and registers it in both dispatch tables.

A semantic handler uses the VM's current frame and the existing exception and suspension helpers:

```rust,ignore
LoadNone => op_load_none {
    vm.push(Value::None);
}
```

Falling through continues execution. The generated wrapper publishes the source position before the body,
then reloads the active frame before dispatching again. Use `try_catch!`, `catch!`,
`handle_load_result!` and `handle_call_result!` for operations that can raise or suspend.

## Adding a fast handler

Select it beside the semantic implementation with `using`:

```rust,ignore
LoadConst => op_load_const using native_constant {
    let idx = vm.current_frame.fetch_u16();
    let value = vm.current_frame.code.constant(idx);
    vm.push(value.clone_with_heap(vm));
}
```

A const-generic family uses `using native_local::<OP>`; the macro supplies the declaring opcode as `OP`.
There is no separate registration table to update.
`with inline` inlines the semantic body into its wrapper; only `BinarySubscr` currently uses this measured exception.

Write direct handlers inside `direct_handler!`. Name the scalar arguments; the macro supplies their types and `Status` return type:

```rust,ignore
direct_handler! {
fn native_nop(vm, bytes, ip, context, remaining, table) {
    next!(1)
}
}
```

Use `mut ip` in the declaration when decoding operands. Const-generic families use
`fn native_local<const OP: u8>(vm, bytes, mut ip, context, remaining, table)`.

The continuation macros expand directly to the scalar calls used by the dispatcher:

| Operation | Meaning |
| --- | --- |
| `vm` | Stack, locals, heap and semantic operations |
| `bytes`, `ip` | Checked bytecode access; entry IP points just after the opcode |
| `next!(WIDTH)` | Continue at `ip`; `WIDTH` includes the opcode byte and operands |
| `next!(WIDTH, target)` | Continue at a supplied instruction pointer |
| `next!(WIDTH, target, budget)` | Continue after accounting for fused instructions |
| `next_local!(OP, target)` | Select the width of a fixed, narrow or wide local opcode |
| `fallback!(OP)` | Run the semantic handler with the original operands and entry IP |
| `resume!()` | Reload the active frame after Python execution or a frame change |
| `context` | Record an exit or uncaught error payload |

Advance `ip` past decoded operands, or supply the target explicitly. `WIDTH` reconstructs the source position at a checkpoint;
it does not advance the instruction pointer. Return the continuation's status immediately, as a tail expression or with `return`.

## State and ownership

Call `fallback!` before consuming stack operands or advancing `ip`; the semantic handler decodes them itself.
If an operation can run Python, publish `vm.current_frame.ip` and `vm.instruction_ip` before the call, then use `resume!()`.
Direct handlers that cannot run Python can retain instruction state locally until a checkpoint.

For a branch, set `vm.instruction_ip` to the source opcode when the countdown reaches zero, then use `next!(0)`.
The branch target cannot reconstruct the source position.
For fused operations, account for every consumed opcode and preserve a checkpoint that falls between them.
`for_iter` demonstrates the `ForIter` / `StoreLocal` case.

Keep the usual `Value` clone/drop ownership rules. A fast path must leave operands untouched when it declines an operation.
Keep operation-local guards out of the continuation: release them before dispatching to another handler.

After changing a fast handler, run the dispatch tests and inspect release assembly or benchmarks.
The NOP handler should still finish with an indirect tail branch; an aggregate argument or an outlined continuation
can add memory traffic even when the source looks shorter.
