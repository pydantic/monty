from typing import reveal_type

# === Core types ===
reveal_type(None)
reveal_type(object())
reveal_type(type(1))

# === Primitive types ===
reveal_type(True)
reveal_type(int(1))
reveal_type(float(1.2))

# === String/bytes types ===
reveal_type('hello')
reveal_type(b'foobar')

# === Container types ===
reveal_type([1])
reveal_type((1, 2))
reveal_type({1: 2})
reveal_type({1, 2})
reveal_type(frozenset({1, 2}))
reveal_type(range(10))

# === Iterator types ===
reveal_type(enumerate([1, 2]))
reveal_type(reversed([1, 2]))
reveal_type(zip([1], [2]))

# === Slicing ===
reveal_type(slice(1, 2))

# === Exception types ===
reveal_type(BaseException())
reveal_type(Exception())
reveal_type(SystemExit())
reveal_type(KeyboardInterrupt())
reveal_type(ArithmeticError())
reveal_type(OverflowError())
reveal_type(ZeroDivisionError())
reveal_type(LookupError())
reveal_type(IndexError())
reveal_type(KeyError())
reveal_type(RuntimeError())
reveal_type(NotImplementedError())
reveal_type(RecursionError())
reveal_type(AttributeError())
reveal_type(AssertionError())
reveal_type(MemoryError())
reveal_type(NameError())
reveal_type(SyntaxError())
reveal_type(OSError())
reveal_type(TimeoutError())
reveal_type(TypeError())
reveal_type(ValueError())
reveal_type(StopIteration())
