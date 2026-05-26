from dataclasses import dataclass, is_dataclass
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import MemoryFile, OSAccess


def test_monty_dump_load_roundtrip():
    m = pydantic_monty.Monty('x + 1', inputs=['x'])
    data = m.dump()

    assert isinstance(data, bytes)
    assert len(data) > 0

    m2 = pydantic_monty.Monty.load(data)
    assert m2.run(inputs={'x': 41}) == snapshot(42)


def test_monty_dump_load_preserves_script_name():
    m = pydantic_monty.Monty('1', script_name='custom.py')
    data = m.dump()

    m2 = pydantic_monty.Monty.load(data)
    assert repr(m2) == snapshot("Monty(<1 line of code>, script_name='custom.py')")


def test_monty_dump_load_preserves_inputs():
    m = pydantic_monty.Monty('x + y', inputs=['x', 'y'])
    data = m.dump()

    m2 = pydantic_monty.Monty.load(data)
    assert m2.run(inputs={'x': 1, 'y': 2}) == snapshot(3)


def test_monty_dump_load_preserves_external_functions():
    m = pydantic_monty.Monty('func()')
    data = m.dump()

    m2 = pydantic_monty.Monty.load(data)
    result = m2.run(external_functions={'func': lambda: 42})
    assert result == snapshot(42)


def test_monty_load_invalid_data():
    with pytest.raises(ValueError) as exc_info:
        pydantic_monty.Monty.load(b'invalid data')
    assert str(exc_info.value) == snapshot('Hit the end of buffer, expected more data')


def test_progress_dump_load_roundtrip():
    m = pydantic_monty.Monty('func(1, 2)')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    assert isinstance(data, bytes)
    assert len(data) > 0

    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.function_name == snapshot('func')
    assert progress2.args == snapshot((1, 2))
    assert progress2.kwargs == snapshot({})

    result = progress2.resume({'return_value': 100})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(100)


def test_progress_dump_load_preserves_script_name():
    m = pydantic_monty.Monty('func()', script_name='test.py')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.script_name == snapshot('test.py')


def test_progress_dump_load_with_kwargs():
    m = pydantic_monty.Monty('func(a=1, b="hello")')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.function_name == snapshot('func')
    assert progress2.args == snapshot(())
    assert progress2.kwargs == snapshot({'a': 1, 'b': 'hello'})


def test_progress_dump_after_resume_fails():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    progress.resume({'return_value': 1})

    with pytest.raises(RuntimeError) as exc_info:
        progress.dump()
    assert exc_info.value.args[0] == snapshot('Cannot dump progress that has already been resumed')


def test_progress_load_invalid_data():
    with pytest.raises(ValueError):
        pydantic_monty.load_snapshot(b'invalid data')


def test_progress_dump_load_multiple_calls():
    m = pydantic_monty.Monty('a() + b()')

    # First call
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('a')

    # Dump and load the state
    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    # Resume with first return value
    progress3 = progress2.resume({'return_value': 10})
    assert isinstance(progress3, pydantic_monty.FunctionSnapshot)
    assert progress3.function_name == snapshot('b')

    # Dump and load again
    data2 = progress3.dump()
    progress4 = pydantic_monty.load_snapshot(data2)
    assert isinstance(progress4, pydantic_monty.FunctionSnapshot)

    # Resume with second return value
    result = progress4.resume({'return_value': 5})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(15)


def test_progress_load_with_print_callback():
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    m = pydantic_monty.Monty('print("before"); func(); print("after")')
    progress = m.start(print_callback=callback)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert output == snapshot([('stdout', 'before'), ('stdout', '\n')])

    # Dump and load with new callback
    data = progress.dump()
    output.clear()
    progress2 = pydantic_monty.load_snapshot(data, print_callback=callback)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert output == snapshot([('stdout', 'after'), ('stdout', '\n')])


def test_progress_load_without_print_callback():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    result = progress2.resume({'return_value': 42})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(42)


@pytest.mark.parametrize(
    'code,expected',
    [
        ('1 + 1', 2),
        ('"hello"', 'hello'),
        ('[1, 2, 3]', [1, 2, 3]),
        ('{"a": 1}', {'a': 1}),
        ('True', True),
        ('None', None),
    ],
)
def test_monty_dump_load_various_outputs(code: str, expected: Any):
    m = pydantic_monty.Monty(code)
    data = m.dump()
    m2 = pydantic_monty.Monty.load(data)
    assert m2.run() == expected


def test_progress_dump_load_with_limits():
    m = pydantic_monty.Monty('func()')
    limits = pydantic_monty.ResourceLimits(max_allocations=1000)
    progress = m.start(limits=limits)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    result = progress2.resume({'return_value': 99})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(99)


@dataclass
class Person:
    name: str
    age: int


def test_monty_load_dataclass():
    m = pydantic_monty.Monty('x', inputs=['x'])
    data = m.dump()

    m2 = pydantic_monty.Monty.load(data)
    m2.register_dataclass(Person)
    result = m2.run(inputs={'x': Person(name='Alice', age=30)})
    assert isinstance(result, Person)


def test_progress_dump_load_dataclass():
    m = pydantic_monty.Monty('func()')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    assert isinstance(data, bytes)
    assert len(data) > 0

    progress2 = pydantic_monty.load_snapshot(data, dataclass_registry=[Person])
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.function_name == snapshot('func')
    assert progress2.args == snapshot(())
    assert progress2.kwargs == snapshot({})

    result = progress2.resume({'return_value': Person(name='Alice', age=30)})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert isinstance(result.output, Person)
    assert result.output.name == snapshot('Alice')
    assert result.output.age == snapshot(30)


def test_progress_dump_load_unknown_dataclass():
    """When a snapshot containing a dataclass is loaded without registering the type,
    the result should be an UnknownDataclass with the correct attributes."""
    m = pydantic_monty.Monty(
        'external_call()\nx',
        inputs=['x'],
    )
    progress = m.start(inputs={'x': Person(name='Bob', age=25)})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('external_call')

    # Dump the snapshot (dataclass x is in the heap)
    data = progress.dump()

    # Load WITHOUT providing dataclass_registry — Person type is unknown
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    # Resume execution — x is returned as UnknownDataclass
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)

    output = result.output
    # Should NOT be a Person instance since the type wasn't registered
    assert not isinstance(output, Person)
    assert type(output).__name__ == snapshot('UnknownDataclass')

    # Attributes should still be accessible
    assert output.name == snapshot('Bob')
    assert output.age == snapshot(25)

    # Should be compatible with dataclasses module
    assert is_dataclass(output)

    # repr should indicate it's unknown
    assert repr(output) == snapshot("<Unknown Dataclass Person(name='Bob', age=25)>")


# =============================================================================
# Open-file buffer survives dump/load
# =============================================================================
#
# These tests pin the snapshot/restore contract for buffered file I/O.
# After `f.read(N)` loads the full-file buffer into a heap-resident `Str` /
# `Bytes` entry, snapshotting and reloading must preserve:
#   - the buffer contents (so subsequent reads don't re-trigger an OS call),
#   - the file's `position` / `eof` state,
#   - the cached `buffer_meta` (or rehydrate it lazily) so character
#     indices keep matching byte indices for UTF-8 content.
#
# Each test uses an external function call (`checkpoint(...)`) to force a
# `FunctionSnapshot` yield *after* the buffer is loaded; without that pause
# the runtime would auto-dispatch all OS calls and run to completion before
# we could observe the dump/load roundtrip.


def test_snapshot_preserves_buffered_read_text():
    """Buffered text read survives dump/load; the second read uses the
    restored buffer instead of re-issuing a `ReadText` OS call."""
    fs = OSAccess([MemoryFile('/data.txt', content='hello world!')])
    code = """
f = open('/data.txt')
first = f.read(5)
checkpoint(first)
second = f.read()
first + ' | ' + second
"""
    m = pydantic_monty.Monty(code)
    progress = m.start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('checkpoint')
    assert progress.args == snapshot(('hello',))

    data = progress.dump()
    progress2 = pydantic_monty.load_snapshot(data)
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)

    # Resume *without* `os=fs`: if the buffer wasn't preserved, the next
    # `f.read()` would need another `ReadText` OS call, which would surface
    # as a follow-up FunctionSnapshot instead of completing.
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot('hello |  world!')


def test_snapshot_preserves_buffered_read_bytes():
    """Same contract for binary mode — the `Bytes` heap entry survives."""
    fs = OSAccess([MemoryFile('/data.bin', content=b'\x00\x01\x02\x03\x04\x05')])
    code = """
f = open('/data.bin', 'rb')
first = f.read(3)
checkpoint(first)
second = f.read()
first + second
"""
    progress = pydantic_monty.Monty(code).start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot((b'\x00\x01\x02',))

    progress2 = pydantic_monty.load_snapshot(progress.dump())
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(b'\x00\x01\x02\x03\x04\x05')


def test_snapshot_preserves_file_position_and_seek():
    """`tell()` after restore matches the position at snapshot time, and
    `seek()` on the restored file repositions within the cached buffer
    without re-issuing an OS call."""
    fs = OSAccess([MemoryFile('/data.txt', content='abcdefghij')])
    code = """
f = open('/data.txt')
f.read(4)  # position -> 4
pos_before = f.tell()
checkpoint(pos_before)
pos_after = f.tell()
f.seek(0)
re_read = f.read(3)
(pos_before, pos_after, re_read)
"""
    progress = pydantic_monty.Monty(code).start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot((4,))

    progress2 = pydantic_monty.load_snapshot(progress.dump())
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    # pos_after must match pos_before (no implicit reset on load); the
    # seek-and-reread must succeed with the restored buffer.
    assert result.output == snapshot((4, 4, 'abc'))


def test_snapshot_preserves_buffer_meta_for_utf8_text():
    """UTF-8 multi-byte content stresses the cached `buffer_meta`
    (`byte_position` ≠ `position`). After restore, sized reads must keep
    advancing the char index in sync with the byte offset."""
    # 'αβγδε' = 5 chars, 10 bytes (each is 2 UTF-8 bytes).
    fs = OSAccess([MemoryFile('/greek.txt', content='αβγδε')])
    code = """
f = open('/greek.txt')
first = f.read(2)   # 'αβ' — char-index now at 2, byte-index at 4
checkpoint(first)
second = f.read(2)  # 'γδ'
third = f.read()    # 'ε'
(first, second, third, f.tell())
"""
    progress = pydantic_monty.Monty(code).start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot(('αβ',))

    progress2 = pydantic_monty.load_snapshot(progress.dump())
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    # `tell()` in text mode is the char index; if buffer_meta failed to
    # rehydrate, the subsequent reads would slice on the wrong byte boundary
    # and produce mojibake or panic.
    assert result.output == snapshot(('αβ', 'γδ', 'ε', 5))


def test_snapshot_buffer_survives_double_roundtrip():
    """Two dump/load roundtrips on the same buffered file — the buffer
    entry must remain consistent across multiple serialisation passes."""
    fs = OSAccess([MemoryFile('/data.txt', content='0123456789')])
    code = """
f = open('/data.txt')
a = f.read(2)
checkpoint(a)
b = f.read(2)
checkpoint(b)
c = f.read()
a + '-' + b + '-' + c
"""
    progress = pydantic_monty.Monty(code).start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot(('01',))

    # First roundtrip — resume past the first checkpoint.
    progress2 = pydantic_monty.load_snapshot(progress.dump())
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    progress3 = progress2.resume({'return_value': None})
    assert isinstance(progress3, pydantic_monty.FunctionSnapshot)
    assert progress3.args == snapshot(('23',))

    # Second roundtrip — buffer must still match the restored position.
    progress4 = pydantic_monty.load_snapshot(progress3.dump())
    assert isinstance(progress4, pydantic_monty.FunctionSnapshot)
    result = progress4.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot('01-23-456789')


def test_snapshot_after_close_does_not_repopulate_buffer():
    """A file closed before snapshotting stays closed (and bufferless)
    after restore — accidentally rehydrating its buffer would re-admit
    memory the user asked us to release."""
    fs = OSAccess([MemoryFile('/data.txt', content='hello')])
    code = """
f = open('/data.txt')
data = f.read()
f.close()
checkpoint(data)
# Touching `f` again must still raise — the close persisted across the snapshot.
try:
    f.read()
    result = 'no error'
except ValueError as e:
    result = str(e)
(data, result)
"""
    progress = pydantic_monty.Monty(code).start(os=fs)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot(('hello',))

    progress2 = pydantic_monty.load_snapshot(progress.dump())
    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    result = progress2.resume({'return_value': None})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(('hello', 'I/O operation on closed file.'))
