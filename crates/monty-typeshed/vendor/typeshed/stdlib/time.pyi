# Monty's `time` module: the two functions and four zone constants it implements
# at runtime (see `crates/monty/src/modules/time.rs`). Upstream typeshed's
# `stdlib/time.pyi` is not vendored — the monotonic clocks, `struct_time` and
# the conversion functions have no runtime behind them.

timezone: int
altzone: int
daylight: int
tzname: tuple[str, str]

def time() -> float: ...
def sleep(seconds: float, /) -> None: ...
