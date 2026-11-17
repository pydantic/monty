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

Fast handlers in [`fast.rs`](../../../monty/src/bytecode/vm/dispatch_threaded/fast.rs) are ordinary Rust functions
that take a `Dispatch` value:

```rust,ignore
#[inline(always)]
pub(super) fn native_nop(d: Dispatch<'_, '_>) -> Status {
    d.next()
}
```

Use `mut d` when advancing the bytecode cursor or accounting for fused instructions.
Const-generic families use `pub(super) fn native_local<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status`.
The registration macro generates a scalar entry point and inlines the function into it.
`Dispatch` stays local to that entry point; it is never passed between handlers.

| Operation                     | Meaning                                                            |
| ----------------------------- | ------------------------------------------------------------------ |
| `d.vm`                        | Stack, locals, heap and semantic operations                        |
| `d.read_u8()`, `d.read_u16()` | Read an operand and advance the bytecode cursor                    |
| `d.bytes`, `d.ip`             | Look ahead without consuming an operand; set a branch target       |
| `d.next()`                    | Continue at the cursor, retaining this instruction's source offset |
| `d.fallback(OP)`              | Run the semantic handler from the original operand position        |
| `d.resume()`                  | Reload the active frame after Python execution or a frame change   |
| `d.remaining`                 | Instruction budget; subtract each additional fused instruction     |
| `d.context`                   | Record an exit or uncaught error payload                           |

Operand readers advance `d.ip`; `next()` does not.
Continuation methods consume `Dispatch`, so return their result as a tail expression or with `return`.
Handlers do not need to supply instruction widths or select a checkpoint function.

Subscription operands use the shared types in
[`subscription.rs`](../../../monty/src/bytecode/subscription.rs).
Their constructors return `None` when a slot cannot fit the fused encoding, so the compiler emits ordinary loads.
The compiler, builder and handlers share these encoders and decoders.

## State and ownership

Call `d.fallback(OP)` before consuming stack operands; it restores the original operand position even after decoding.
If an operation can run Python, publish `d.vm.current_frame.ip` and `d.vm.instruction_ip` before the call,
then use `d.resume()`.
Direct handlers that cannot run Python can retain instruction state locally until a checkpoint.

`d.next()` publishes the source offset when the countdown reaches zero, including after a branch.
For fused operations, account for every consumed opcode and preserve a checkpoint that falls between them.
`for_iter` demonstrates the `ForIter` / `StoreLocal` case.
Fused subscriptions also account for skipped key loads and stores.
When folding `StoreLocal` into `ForIter`, set `d.start` to the store's opcode position before advancing the cursor.
This attributes a checkpoint between loop iterations to the store, matching execution without fusion.

Keep the usual `Value` clone/drop ownership rules. A fast path must leave operands untouched when it declines an operation.
Keep operation-local guards out of the continuation: release them before dispatching to another handler.
Construct temporary literal keys outside the handler when passing their addresses to lookup helpers would block tail-call elimination.

After changing a fast handler, run the dispatch tests and inspect release assembly or benchmarks.
The NOP handler should still finish with an indirect tail branch; an aggregate argument or an outlined continuation
can add memory traffic even when the source looks shorter.
