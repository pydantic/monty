# Monty's `time` module: only the two functions it implements at runtime (see
# `crates/monty/src/modules/time.rs`). Upstream typeshed's `stdlib/time.pyi`
# is not vendored — the monotonic clocks, `struct_time` and the conversion
# functions have no runtime behind them.

def time() -> float: ...
def sleep(seconds: float, /) -> None: ...
