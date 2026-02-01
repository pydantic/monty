import pytest
from inline_snapshot import snapshot

from pydantic_monty import MemoryFile, Monty, MontyRuntimeError, OSAccess


def test_basics():
    fs = OSAccess([MemoryFile('/test/file.txt', content='foo')])

    result = Monty('from pathlib import Path; Path("/test/file.txt").exists()').run(os=fs)
    assert result == snapshot(True)

    result = Monty('from pathlib import Path; Path("/test/other.txt").exists()').run(os=fs)
    assert result == snapshot(False)

    result = Monty('from pathlib import Path; Path("/test/file.txt").read_text()').run(os=fs)
    assert result == snapshot('foo')

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty('from pathlib import Path; Path("/test/other.txt").read_text()').run(os=fs)

    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/test/other.txt'")
    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "main.py", line 1, in <module>
    from pathlib import Path; Path("/test/other.txt").read_text()
                              ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
FileNotFoundError: [Errno 2] No such file or directory: '/test/other.txt'\
""")
