from pathlib import Path, PurePosixPath

import pytest
from conftest import RunMonty

from pydantic_monty import CallbackFile, MemoryFile, MontyFileHandle, MountDir, OSAccess


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
    f.seek(8)
    f.write(b'Z')
    f.seek(100)
    assert f.write(b'') == 0
    assert f.seek(0, 2) == 9
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
    assert result == b'abXYef\0\0Z'


def test_memory_positional_write_limit_precedes_allocation():
    fs = OSAccess([MemoryFile('/file', b'abc')], max_total_file_bytes=8)
    with pytest.raises(OSError):
        fs.path_write_bytes_at(PurePosixPath('/file'), 100_000_000, b'x')
    assert fs.path_read_bytes(PurePosixPath('/file')) == b'abc'


def test_callback_positional_write_limit_precedes_gap_allocation():
    writes: list[int] = []
    fs = OSAccess(
        [CallbackFile('/file', read=lambda _: b'', write=lambda _, data: writes.append(len(data)))],
        max_total_file_bytes=8,
    )
    with pytest.raises(OSError):
        fs.path_write_bytes_at(PurePosixPath('/file'), 1_000_000, b'x')
    assert writes == []


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
def test_invalid_host_write_counts_do_not_advance_cursor(monty_run: RunMonty, count: int):
    class InvalidOS(OSAccess):
        def path_write_bytes_at(self, path: PurePosixPath | MontyFileHandle, offset: int, data: bytes) -> int:
            return count

    assert (
        monty_run(
            """
f = open('/file', 'w+b')
try:
    f.write(b'abc')
except ValueError:
    pass
else:
    assert False
f.tell()
""",
            os=InvalidOS([MemoryFile('/file', b'')]),
        )
        == 0
    )
