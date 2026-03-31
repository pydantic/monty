# mount-fs
import sys
from pathlib import Path

# root is injected by the test runner:
# - Monty: Path('/mnt') with OverlayMemory mount over a real temp directory
# - CPython: Path('<real_tmpdir>') pointing to a real temp directory

is_monty = sys.platform == 'monty'

# === FileNotFoundError on read_text of nonexistent ===
try:
    (root / 'nonexistent.txt').read_text()
    assert False, 'expected FileNotFoundError on read_text'
except FileNotFoundError as exc:
    assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc!r}'

# === FileNotFoundError on read_bytes of nonexistent ===
try:
    (root / 'nonexistent.bin').read_bytes()
    assert False, 'expected FileNotFoundError on read_bytes'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.bin'", f'unexpected message: {exc!r}'

# === FileNotFoundError on unlink of nonexistent ===
try:
    (root / 'nonexistent.txt').unlink()
    assert False, 'expected FileNotFoundError on unlink'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'

# === FileNotFoundError on stat of nonexistent ===
try:
    (root / 'nonexistent.txt').stat()
    assert False, 'expected FileNotFoundError on stat'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'

# === Error on mkdir without parents when parent missing ===
try:
    (root / 'missing_parent' / 'child').mkdir()
    assert False, 'expected error on mkdir without parents'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/missing_parent/child'", (
            f'unexpected message: {exc}'
        )

# === FileExistsError on mkdir of existing dir without exist_ok ===
try:
    (root / 'subdir').mkdir()
    assert False, 'expected FileExistsError on mkdir existing'
except FileExistsError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 17] File exists: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 17] File exists: '/mnt/subdir'", f'unexpected message: {exc}'

# === Error on rmdir of non-empty directory ===
try:
    (root / 'subdir').rmdir()
    assert False, 'expected error on rmdir non-empty'
except OSError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith(("[Errno 66] Directory not empty: '", "[Errno 39] Directory not empty: '")), (
            f'exc message: {exc}'
        )
    if is_monty:
        assert str(exc) == "[Errno 39] Directory not empty: '/mnt/subdir'", f'unexpected message: {exc}'

# === FileNotFoundError on rmdir of nonexistent ===
try:
    (root / 'nonexistent_dir').rmdir()
    assert False, 'expected FileNotFoundError on rmdir nonexistent'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent_dir'", f'unexpected message: {exc}'

# === UnicodeDecodeError on read_text of non-UTF-8 file ===
(root / 'bad_utf8.bin').write_bytes(b'\x80\x81\x82')
try:
    (root / 'bad_utf8.bin').read_text()
    assert False, 'expected UnicodeDecodeError on read_text of non-UTF-8'
except UnicodeDecodeError as exc:
    if sys.platform != 'win32':
        assert str(exc) == "'utf-8' codec can't decode byte 0x80 in position 0: invalid start byte", (
            f'unexpected message: {exc}'
        )

# === FileNotFoundError on write_text with missing parent ===
try:
    (root / 'no_such_parent' / 'child.txt').write_text('should fail')
    assert False, 'expected FileNotFoundError on write_text with missing parent'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/no_such_parent/child.txt'", (
            f'unexpected message: {exc}'
        )

# === FileNotFoundError on write_bytes with missing parent ===
try:
    (root / 'no_such_parent' / 'child.bin').write_bytes(b'should fail')
    assert False, 'expected FileNotFoundError on write_bytes with missing parent'
except FileNotFoundError as exc:
    if sys.platform != 'win32':
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/no_such_parent/child.bin'", (
            f'unexpected message: {exc}'
        )

# === TypeError on write_text with wrong argument type ===
try:
    (root / 'hello.txt').write_text(123)
    assert False, 'expected TypeError on write_text with int'
except TypeError as exc:
    assert str(exc) == 'data must be str, not int', f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_text()
    assert False, 'expected TypeError on write_text with int'
except TypeError as exc:
    assert str(exc) == "Path.write_text() missing 1 required positional argument: 'data'", f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_bytes(123)
    assert False, 'expected TypeError on write_bytes with int'
except TypeError as exc:
    assert str(exc) == "memoryview: a bytes-like object is required, not 'int'", f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_bytes()
    assert False, 'expected TypeError on write_bytes with int'
except TypeError as exc:
    assert str(exc) == "Path.write_bytes() missing 1 required positional argument: 'data'", f'unexpected message: {exc}'
