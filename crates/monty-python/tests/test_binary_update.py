from pathlib import Path, PurePosixPath

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import CallbackFile, MemoryFile, MontyFileHandle, MontyRuntimeError, MountDir, OSAccess


@pytest.mark.parametrize('mode', ['w+b', 'wb+', '+bw'])
@pytest.mark.parametrize('backend', ['memory', 'native', 'overlay'])
def test_binary_update(monty_run: RunMonty, tmp_path: Path, mode: str, backend: str):
    code = f"""
from pathlib import Path
with Path('/mnt/file').open({mode!r}) as f:
    assert f.read() == b''
    f.write(b'abcdef')
    f.seek(0)
    old = f.read()
    f.seek(2)
    f.write(b'XY')
    assert f.read() == b'ef'
    f.write(b'Z')
    f.seek(100)
    assert f.write(b'') == 0
    assert f.seek(0, 2) == 7
    f.seek(0)
    assert old == b'abcdef'
    result = f.read()
result
"""
    if backend == 'memory':
        result = monty_run(code, os=OSAccess([MemoryFile('/mnt/file', b'old')]))
    else:
        (tmp_path / 'file').write_bytes(b'old')
        mount_mode = 'overlay' if backend == 'overlay' else 'read-write'
        result = monty_run(code, mount=MountDir(virtual_path='/mnt', host_path=tmp_path, mode=mount_mode))
        if backend == 'overlay':
            assert (tmp_path / 'file').read_bytes() == b'old'
    assert result == b'abXYefZ'


def test_memory_positional_write_limit_precedes_allocation():
    fs = OSAccess([MemoryFile('/file', b'abc')], max_total_file_bytes=8)
    with pytest.raises(OSError):
        fs.path_write_bytes_at(PurePosixPath('/file'), 3, b'x' * 6)
    assert fs.path_read_bytes(PurePosixPath('/file')) == b'abc'


@pytest.mark.parametrize('kind', ['memory', 'callback'])
def test_positional_writes_past_eof_are_rejected(kind: str):
    writes: list[int] = []
    if kind == 'memory':
        file = MemoryFile('/file', b'abc')
    else:
        file = CallbackFile('/file', read=lambda _: b'abc', write=lambda _, data: writes.append(len(data)))
    fs = OSAccess([file])
    for offset in [4, 2**40]:
        with pytest.raises(ValueError) as exc_info:
            fs.path_write_bytes_at(PurePosixPath('/file'), offset, b'x')
        assert exc_info.value.args[0] == snapshot('write offset is past the end of the file')
    assert fs.path_write_bytes_at(PurePosixPath('/file'), 2**40, b'') == 0
    assert writes == []
    if kind == 'memory':
        assert fs.path_read_bytes(PurePosixPath('/file')) == b'abc'


def test_binary_update_handle_mode():
    handle = MontyFileHandle('/file', 'w+b')
    assert handle.mode == 'wb+'
    assert handle.binary and handle.readable and handle.writable


def test_partial_host_error_invalidates_cached_reads(monty_run: RunMonty):
    class PartialOS(OSAccess):
        def path_write_bytes_at(self, path: PurePosixPath | MontyFileHandle, offset: int, data: bytes) -> int:
            count = super().path_write_bytes_at(path, offset, data)
            if data == b'X':
                raise OSError('failed after mutation')
            return count

    result = monty_run(
        """
f = open('/file', 'w+b')
f.write(b'abc')
f.seek(0)
old = f.read()
f.seek(1)
try:
    f.write(b'X')
except OSError:
    pass
assert f.tell() == 1
f.seek(0)
(old, f.read())
""",
        os=PartialOS([MemoryFile('/file', b'')]),
    )
    assert result == (b'abc', b'aXc')


@pytest.mark.parametrize('count', [-1, 4])
def test_invalid_host_write_counts_cannot_be_caught_in_the_sandbox(monty_run: RunMonty, count: int):
    class InvalidOS(OSAccess):
        def path_write_bytes_at(self, path: PurePosixPath | MontyFileHandle, offset: int, data: bytes) -> int:
            return count

    with pytest.raises(MontyRuntimeError, match='Internal error in monty: write count'):
        monty_run(
            """
f = open('/file', 'w+b')
try:
    f.write(b'abc')
except Exception:
    pass
else:
    assert False
f.tell()
""",
            os=InvalidOS([MemoryFile('/file', b'')]),
        )


@pytest.mark.parametrize('mode', ['wb', 'w+b'])
def test_seek_end_refreshes_length_after_partial_write_error(monty_run: RunMonty, mode: str):
    class PartialOS(OSAccess):
        def path_write_bytes_at(self, path: PurePosixPath | MontyFileHandle, offset: int, data: bytes) -> int:
            if data == b'XYZ':
                super().path_write_bytes_at(path, offset, data[:1])
                raise OSError('failed after writing a prefix')
            return super().path_write_bytes_at(path, offset, data)

    result = monty_run(
        f"""
from pathlib import Path
f = open('/file', {mode!r})
f.write(b'abcdefgh')
try:
    f.write(b'XYZ')
except OSError:
    pass
assert f.tell() == 8
assert f.seek(-1, 2) == 8
assert f.seek(0, 2) == 9
f.write(b'!')
f.close()
Path('/file').read_bytes()
""",
        os=PartialOS([MemoryFile('/file', b'')]),
    )
    assert result == b'abcdefghX!'
