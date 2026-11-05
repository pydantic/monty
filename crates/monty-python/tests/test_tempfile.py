"""Named temporary files use the ordinary file object and host filesystem boundary."""

from pathlib import Path, PurePosixPath
from typing import Literal

import pytest
from conftest import RunMonty

from pydantic_monty import MemoryFile, MontyFileHandle, MontyRuntimeError, MountDir, OSAccess


@pytest.mark.parametrize('mode', ['w', 'wb', 'a', 'ab', 'wt', 'at', 'w+b', 'wb+'])
def test_named_file_lifecycle(monty_run: RunMonty, mode: str):
    osa = OSAccess([MemoryFile('/tmp/anchor', '')])
    result = monty_run(
        """
import tempfile
from pathlib import Path
with tempfile.NamedTemporaryFile(mode=mode, prefix='report-', suffix='.txt', delete_on_close=False) as f:
    name = f.name
    f.write(content)
    f.write(content)
    f.close()
    exists_after_close = Path(name).exists()
    contents = Path(name).read_bytes()
(f.closed, exists_after_close, Path(name).exists(), name, contents)
""",
        inputs={'mode': mode, 'content': b'hello' if 'b' in mode else 'hello'},
        os=osa,
    )
    closed, after_close, after_exit, name, contents = result
    assert (closed, after_close, after_exit, contents) == (True, True, False, b'hellohello')
    assert name.startswith('/tmp/report-') and name.endswith('.txt')
    random_name = name.removeprefix('/tmp/report-').removesuffix('.txt')
    assert len(random_name) == 8
    assert all(char in 'abcdefghijklmnopqrstuvwxyz0123456789_' for char in random_name)


@pytest.mark.parametrize('mode', [None, 'w+b', 'wb+'])
@pytest.mark.parametrize('storage', ['memory', 'read-write', 'overlay'])
def test_binary_update_mode(
    monty_run: RunMonty, tmp_path: Path, mode: str | None, storage: Literal['memory', 'read-write', 'overlay']
):
    options = {} if mode is None else {'mode': mode}
    code = """
import tempfile
from pathlib import Path
with tempfile.NamedTemporaryFile(delete_on_close=False, **options) as f:
    f.write(b'abcdef')
    f.seek(0)
    initial = f.read()
    f.seek(2)
    f.write(b'XY')
    f.seek(0)
    overwritten = f.read()
    f.seek(8)
    f.write(b'!')
    f.seek(0)
    extended = f.read()
    mode = f.mode
    f.close()
    exists_after_close = Path(f.name).exists()
(initial, overwritten, extended, mode, exists_after_close, Path(f.name).exists())
"""
    result = None
    if storage == 'memory':
        result = monty_run(code, inputs={'options': options}, os=OSAccess([MemoryFile('/tmp/anchor', '')]))
    else:
        with MountDir(host_path=tmp_path, virtual_path='/tmp', mode=storage) as mount:
            result = monty_run(code, inputs={'options': options}, mount=mount)
        assert list(tmp_path.iterdir()) == []
    assert result == (b'abcdef', b'abXYef', b'abXYef\x00\x00!', 'wb+', True, False)


def test_cleanup_when_body_raises(monty_run: RunMonty):
    assert monty_run(
        """
import tempfile
from pathlib import Path
try:
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:
        f.write('saved')
        raise ValueError('body')
except ValueError as error:
    message = str(error)
(f.closed, Path(f.name).exists(), message)
""",
        os=OSAccess([MemoryFile('/tmp/anchor', '')]),
    ) == (True, False, 'body')


@pytest.mark.parametrize('use_context', [False, True])
def test_delete_false_preserves_file(monty_run: RunMonty, use_context: bool):
    assert (
        monty_run(
            """
import tempfile
from pathlib import Path
f = tempfile.NamedTemporaryFile(mode='w', delete=False)
if use_context:
    with f:
        f.write('retained')
else:
    f.write('retained')
    f.close()
Path(f.name).read_text()
""",
            inputs={'use_context': use_context},
            os=OSAccess([MemoryFile('/tmp/anchor', '')]),
        )
        == 'retained'
    )


@pytest.mark.parametrize(
    'options, message',
    [
        ('', 'delete_on_close=False'),
        ("mode='w+b'", 'delete_on_close=False'),
        ("mode='r+b', delete_on_close=False", 'update modes'),
        ("mode='r', delete_on_close=False", 'write or append'),
        ("mode='w'", 'delete_on_close=False'),
        ("mode='w', delete_on_close=True", 'delete_on_close=False'),
        ("mode='w', delete_on_close=False, prefix='../'", 'prefix must not contain'),
        ("mode='w', delete_on_close=False, suffix='/x'", 'suffix must not contain'),
        ("mode='w', delete_on_close=False, prefix=b'x'", 'prefix must be str'),
        ("mode='w', delete_on_close=False, dir=b'/tmp'", 'dir must be str'),
        ("mode='w', delete_on_close=False, buffering=0", 'buffering'),
        ("mode='w', delete_on_close=False, encoding='ascii'", 'encoding'),
        ("mode='w', delete_on_close=False, errors='ignore'", 'errors'),
        ("mode='w', delete_on_close=False, newline='invalid'", 'illegal newline'),
        ("mode='wb', delete_on_close=False, newline='\\n'", 'binary mode'),
        ("mode='w', delete_on_close=False, dir='\\x00'", 'null byte'),
    ],
)
def test_invalid_options_precede_creation(monty_run: RunMonty, options: str, message: str):
    with pytest.raises(MontyRuntimeError, match=message):
        monty_run(
            f'import tempfile; tempfile.NamedTemporaryFile({options})', os=OSAccess([MemoryFile('/tmp/anchor', '')])
        )


class Collisions(OSAccess):
    """Reject a selected number of reservations and record each candidate."""

    def __init__(self, failures: int):
        super().__init__([MemoryFile('/tmp/anchor', '')])
        self.failures = failures
        self.candidates: list[PurePosixPath] = []

    def path_create_exclusive(self, path: PurePosixPath, mode: str, *, permissions: int) -> MontyFileHandle:
        self.candidates.append(path)
        if len(self.candidates) <= self.failures:
            raise FileExistsError(str(path))
        return super().path_create_exclusive(path, mode, permissions=permissions)


@pytest.mark.parametrize('failures', [0, 2, 19, 20])
def test_collision_retries_are_bounded(monty_run: RunMonty, failures: int):
    osa = Collisions(failures)
    code = "import tempfile; tempfile.NamedTemporaryFile(mode='w', delete=False).close()"
    if failures == 20:
        with pytest.raises(MontyRuntimeError, match='20 attempts'):
            monty_run(code, os=osa)
    else:
        monty_run(code, os=osa)
    assert len(osa.candidates) == min(failures + 1, 20)
    assert len(set(osa.candidates)) == len(osa.candidates)


def test_paths_and_newlines(monty_run: RunMonty):
    osa = OSAccess([MemoryFile('/work/sub/anchor', '')])
    assert monty_run(
        """
import tempfile
from pathlib import Path
with tempfile.NamedTemporaryFile(mode='w', dir=Path('sub'), newline='\\r\\n', delete_on_close=False) as f:
    f.write('a\\nb')
    contents = Path(f.name).read_bytes()
    absolute = f.name.startswith('/work/sub/')
(contents, absolute)
""",
        cwd='/work',
        os=osa,
    ) == (b'a\r\nb', True)


@pytest.mark.parametrize('mode', ['read-write', 'overlay', 'read-only'])
def test_native_mounts(monty_run: RunMonty, tmp_path: Path, mode: Literal['read-write', 'overlay', 'read-only']):
    with MountDir(host_path=tmp_path, virtual_path='/tmp', mode=mode) as mount:
        code = "import tempfile; f = tempfile.NamedTemporaryFile(mode='wb', delete=False); f.write(b'disk'); f.close(); f.name"
        if mode == 'read-only':
            with pytest.raises(MontyRuntimeError, match='PermissionError'):
                monty_run(code, mount=mount)
        else:
            name = monty_run(code, mount=mount)
            if mode == 'read-write':
                assert (tmp_path / PurePosixPath(name).name).read_bytes() == b'disk'
            else:
                assert list(tmp_path.iterdir()) == []


def test_no_storage_does_not_fall_back(monty_run: RunMonty):
    assert monty_run('import tempfile; (tempfile.gettempdir(), tempfile.gettempprefix())') == ('/tmp', 'tmp')
    with pytest.raises(MontyRuntimeError, match='PermissionError'):
        monty_run("import tempfile; tempfile.NamedTemporaryFile(mode='w', delete=False)")


class RefuseCleanup(OSAccess):
    """A deletion failure must not reopen the file or suppress the error."""

    def path_unlink(self, path: PurePosixPath) -> None:
        raise PermissionError('cleanup refused')


def test_cleanup_failure_leaves_file_closed(monty_run: RunMonty):
    assert monty_run(
        """
import tempfile
try:
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:
        pass
except PermissionError as error:
    result = (f.closed, str(error))
result
""",
        os=RefuseCleanup([MemoryFile('/tmp/anchor', '')]),
    ) == (True, 'cleanup refused')


@pytest.mark.parametrize('mode', ['read-write', 'overlay'])
def test_mount_cleanup_on_exception(monty_run: RunMonty, tmp_path: Path, mode: Literal['read-write', 'overlay']):
    with MountDir(host_path=tmp_path, virtual_path='/tmp', mode=mode) as mount:
        assert monty_run(
            """
import tempfile
from pathlib import Path
try:
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:
        f.write('data')
        raise ValueError('body')
except ValueError:
    pass
(f.closed, Path(f.name).exists())
""",
            mount=mount,
        ) == (True, False)
    assert list(tmp_path.iterdir()) == []


def test_cleanup_on_return_and_break(monty_run: RunMonty):
    assert monty_run(
        """
import tempfile
from pathlib import Path

def work():
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:
        return f.name

returned_name = work()
for i in [1]:
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:
        break
(Path(returned_name).exists(), Path(f.name).exists(), f.closed)
""",
        os=OSAccess([MemoryFile('/tmp/anchor', '')]),
    ) == (False, False, True)


def test_errors_is_keyword_only_before_any_creation(monty_run: RunMonty):
    osa = Collisions(0)
    with pytest.raises(MontyRuntimeError, match='TypeError:.*positional'):
        monty_run(
            "import tempfile; tempfile.NamedTemporaryFile('w', -1, None, None, None, None, None, False, None)",
            os=osa,
        )
    assert osa.candidates == []
    monty_run(
        "import tempfile; tempfile.NamedTemporaryFile('w', -1, None, None, None, None, None, False, errors=None).close()",
        os=osa,
    )
    assert len(osa.candidates) == 1
