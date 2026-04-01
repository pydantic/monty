# mount-fs
import sys
from pathlib import Path

# root is injected by the test runner:
# - Monty: Path('/mnt') with OverlayMemory mount over a real temp directory
# - CPython: Path('<real_tmpdir>') pointing to a real temp directory

is_monty = sys.platform == 'monty'
is_windows = sys.platform == 'win32'

# ============================================================================
# FileNotFoundError — read/write/stat/unlink/rmdir on nonexistent paths
# ============================================================================

# === FileNotFoundError on read_text of nonexistent ===
try:
    (root / 'nonexistent.txt').read_text()
    assert False, 'expected FileNotFoundError on read_text'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on read_bytes of nonexistent ===
try:
    (root / 'nonexistent.bin').read_bytes()
    assert False, 'expected FileNotFoundError on read_bytes'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.bin'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on unlink of nonexistent ===
try:
    (root / 'nonexistent.txt').unlink()
    assert False, 'expected FileNotFoundError on unlink'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on stat of nonexistent ===
try:
    (root / 'nonexistent.txt').stat()
    assert False, 'expected FileNotFoundError on stat'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on stat of deeply nonexistent ===
try:
    (root / 'nonexistent' / 'child.txt').stat()
    assert False, 'expected FileNotFoundError on stat'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent/child.txt'", (
            f'unexpected message: {exc}'
        )
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on rmdir of nonexistent ===
try:
    (root / 'nonexistent_dir').rmdir()
    assert False, 'expected FileNotFoundError on rmdir nonexistent'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent_dir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on rename nonexistent ===
try:
    (root / 'nonexistent.txt').rename(root / 'new.txt')
    assert False, 'expected FileNotFoundError on rename nonexistent'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === Error on mkdir without parents when parent missing ===
try:
    (root / 'missing_parent' / 'child').mkdir()
    assert False, 'expected error on mkdir without parents'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/missing_parent/child'", (
            f'unexpected message: {exc}'
        )
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on write_text with missing parent ===
try:
    (root / 'no_such_parent' / 'child.txt').write_text('should fail')
    assert False, 'expected FileNotFoundError on write_text with missing parent'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/no_such_parent/child.txt'", (
            f'unexpected message: {exc}'
        )
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on write_bytes with missing parent ===
try:
    (root / 'no_such_parent' / 'child.bin').write_bytes(b'should fail')
    assert False, 'expected FileNotFoundError on write_bytes with missing parent'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/no_such_parent/child.bin'", (
            f'unexpected message: {exc}'
        )
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# === FileNotFoundError on iterdir of nonexistent ===
try:
    list((root / 'nonexistent').iterdir())
    assert False, 'expected FileNotFoundError on iterdir nonexistent'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/nonexistent'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# ============================================================================
# FileExistsError — mkdir on existing
# ============================================================================

# === FileExistsError on mkdir of existing dir without exist_ok ===
try:
    (root / 'subdir').mkdir()
    assert False, 'expected FileExistsError on mkdir existing'
except FileExistsError as exc:
    if is_monty:
        assert str(exc) == "[Errno 17] File exists: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 17] File exists: '"), f'exc message: {exc}'

# ============================================================================
# IsADirectoryError — read/write/unlink on directories
# ============================================================================

# === IsADirectoryError on read_text of directory ===
# CPython on Windows raises PermissionError instead of IsADirectoryError
try:
    (root / 'subdir').read_text()
    assert False, 'expected IsADirectoryError on read_text of dir'
except (IsADirectoryError, PermissionError) as exc:
    if is_monty:
        assert str(exc) == "[Errno 21] Is a directory: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 21] Is a directory: '"), f'exc message: {exc}'

# === IsADirectoryError on read_bytes of directory ===
try:
    (root / 'subdir').read_bytes()
    assert False, 'expected IsADirectoryError on read_bytes of dir'
except (IsADirectoryError, PermissionError) as exc:
    if is_monty:
        assert str(exc) == "[Errno 21] Is a directory: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 21] Is a directory: '"), f'exc message: {exc}'

# === IsADirectoryError on write_text to directory ===
try:
    (root / 'subdir').write_text('test')
    assert False, 'expected IsADirectoryError on write_text to dir'
except (IsADirectoryError, PermissionError) as exc:
    if is_monty:
        assert str(exc) == "[Errno 21] Is a directory: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 21] Is a directory: '"), f'exc message: {exc}'

# === IsADirectoryError on write_bytes to directory ===
try:
    (root / 'subdir').write_bytes(b'test')
    assert False, 'expected IsADirectoryError on write_bytes to dir'
except (IsADirectoryError, PermissionError) as exc:
    if is_monty:
        assert str(exc) == "[Errno 21] Is a directory: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 21] Is a directory: '"), f'exc message: {exc}'

# === IsADirectoryError/PermissionError on unlink of directory ===
# macOS returns PermissionError (EPERM), Linux returns IsADirectoryError (EISDIR)
try:
    (root / 'subdir').unlink()
    assert False, 'expected error on unlink of dir'
except (IsADirectoryError, PermissionError):
    pass

# ============================================================================
# NotADirectoryError — iterdir/rmdir on files
# ============================================================================

# === NotADirectoryError on iterdir of file ===
try:
    list((root / 'hello.txt').iterdir())
    assert False, 'expected NotADirectoryError on iterdir of file'
except NotADirectoryError as exc:
    if is_monty:
        assert str(exc) == "[Errno 20] Not a directory: '/mnt/hello.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 20] Not a directory: '"), f'exc message: {exc}'

# === NotADirectoryError on rmdir of file ===
try:
    (root / 'hello.txt').rmdir()
    assert False, 'expected NotADirectoryError on rmdir of file'
except NotADirectoryError as exc:
    if is_monty:
        assert str(exc) == "[Errno 20] Not a directory: '/mnt/hello.txt'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith("[Errno 20] Not a directory: '"), f'exc message: {exc}'

# ============================================================================
# DirectoryNotEmpty — rmdir of non-empty directory
# ============================================================================

# === Error on rmdir of non-empty directory ===
try:
    (root / 'subdir').rmdir()
    assert False, 'expected error on rmdir non-empty'
except OSError as exc:
    if is_monty:
        assert str(exc) == "[Errno 39] Directory not empty: '/mnt/subdir'", f'unexpected message: {exc}'
    elif not is_windows:
        # macOS uses errno 66, Linux uses errno 39
        assert str(exc).startswith(('[Errno 66] Directory not empty:', '[Errno 39] Directory not empty:')), (
            f'exc message: {exc}'
        )

# ============================================================================
# UnicodeDecodeError — read_text of non-UTF-8 file
# ============================================================================

(root / 'bad_utf8.bin').write_bytes(b'\x80\x81\x82')
try:
    (root / 'bad_utf8.bin').read_text()
    assert False, 'expected UnicodeDecodeError on read_text of non-UTF-8'
except UnicodeDecodeError as exc:
    if is_monty:
        assert str(exc) == "'utf-8' codec can't decode byte 0x80 in position 0: invalid start byte", (
            f'unexpected message: {exc}'
        )
    elif not is_windows:
        assert 'utf-8' in str(exc), f'exc message: {exc}'

# ============================================================================
# OSError — path length limits
# ============================================================================

# === OSError on path component too long (> 255 bytes) ===
long_name = 'a' * 256
long_component_path = Path(str(root) + '/' + long_name)
try:
    long_component_path.write_text('test')
    assert False, 'expected OSError on long component'
except OSError as exc:
    if is_monty:
        assert str(exc) == "[Errno 36] File name too long: '/mnt/" + long_name + "'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith(("[Errno 36] File name too long: '", "[Errno 63] File name too long: '")), (
            f'exc message: {exc}'
        )

# === Component at exactly 255 bytes is accepted ===
ok_name = 'b' * 255
assert Path(str(root) + '/' + ok_name).exists() == False, '255-byte component should be accepted'

# === OSError on total path too long (> 4096 bytes) ===
long_path_str = str(root) + '/' + '/'.join(['x' * 200] * 21)
try:
    Path(long_path_str).write_text('test')
    assert False, 'expected OSError on long total path'
except OSError as exc:
    if is_monty:
        assert str(exc).startswith("[Errno 36] File name too long: '"), f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith(("[Errno 36] File name too long: '", "[Errno 63] File name too long: '")), (
            f'exc message: {exc}'
        )

# === OSError on long component in read operations too ===
try:
    long_component_path.read_text()
    assert False, 'expected OSError on read_text with long component'
except OSError as exc:
    if is_monty:
        assert str(exc) == "[Errno 36] File name too long: '/mnt/" + long_name + "'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith(("[Errno 36] File name too long: '", "[Errno 63] File name too long: '")), (
            f'exc message: {exc}'
        )

# === OSError on long component in stat ===
try:
    long_component_path.stat()
    assert False, 'expected OSError on stat with long component'
except OSError as exc:
    if is_monty:
        assert str(exc) == "[Errno 36] File name too long: '/mnt/" + long_name + "'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith(("[Errno 36] File name too long: '", "[Errno 63] File name too long: '")), (
            f'exc message: {exc}'
        )

# === OSError on long component in mkdir ===
try:
    long_component_path.mkdir()
    assert False, 'expected OSError on mkdir with long component'
except OSError as exc:
    if is_monty:
        assert str(exc) == "[Errno 36] File name too long: '/mnt/" + long_name + "'", f'unexpected message: {exc}'
    elif not is_windows:
        assert str(exc).startswith(("[Errno 36] File name too long: '", "[Errno 63] File name too long: '")), (
            f'exc message: {exc}'
        )

# ============================================================================
# TypeError — wrong argument types
# ============================================================================

try:
    (root / 'hello.txt').write_text(123)
    assert False, 'expected TypeError on write_text with int'
except TypeError as exc:
    assert str(exc) == 'data must be str, not int', f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_text()
    assert False, 'expected TypeError on write_text with no args'
except TypeError as exc:
    assert str(exc) == "Path.write_text() missing 1 required positional argument: 'data'", f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_bytes(123)
    assert False, 'expected TypeError on write_bytes with int'
except TypeError as exc:
    assert str(exc) == "memoryview: a bytes-like object is required, not 'int'", f'unexpected message: {exc}'

try:
    (root / 'hello.txt').write_bytes()
    assert False, 'expected TypeError on write_bytes with no args'
except TypeError as exc:
    assert str(exc) == "Path.write_bytes() missing 1 required positional argument: 'data'", f'unexpected message: {exc}'
