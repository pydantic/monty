# mount-fs
# skip-cpython-windows — CPython on Windows defaults text I/O to cp1252 which can't encode β;
# Windows-specific coverage lives in open__fs_windows.py.
import sys

is_monty = sys.platform == 'monty'
is_windows = sys.platform == 'win32'

# === Text read ===
text_file = open(root / 'hello.txt')
assert str(type(text_file)) == "<class '_io.TextIOWrapper'>", 'text open returns TextIOWrapper'
assert text_file.mode == 'r', 'default mode is r'
assert text_file.readable() == True, 'default text file is readable'
assert text_file.writable() == False, 'default text file is not writable'
assert text_file.read() == 'hello world\n', 'text read returns full file'
# Second sequential read should be empty (CPython EOF semantics)
assert text_file.read() == '', 'second text read returns empty after EOF'
assert text_file.read() == '', 'third text read still empty'
# Monty returns False because seek() / tell() are not yet implemented; CPython
# returns True for regular files because they support seeking natively.
expected_seekable = not is_monty
assert text_file.seekable() == expected_seekable, 'seekable() reflects whether seek() is actually implemented'
text_file.close()
assert text_file.closed == True, 'close sets closed'

# === Binary read ===
binary_file = open(root / 'data.bin', 'rb')
assert str(type(binary_file)) == "<class '_io.BufferedReader'>", 'rb open returns BufferedReader'
assert binary_file.mode == 'rb', 'binary mode is preserved'
assert binary_file.read() == b'\x00\x01\x02\x03', 'binary read returns bytes'
assert binary_file.read() == b'', 'second binary read returns empty after EOF'
binary_file.close()

# === Text write ===
writer = open(root / 'open_write.txt', 'w')
assert str(type(writer)) == "<class '_io.TextIOWrapper'>", 'text write returns TextIOWrapper'
assert writer.readable() == False, 'w text file is not readable'
assert writer.writable() == True, 'w text file is writable'
assert writer.write('alpha') == 5, 'text write returns character count'
assert writer.write('\nβ') == 2, 'second text write appends after initial truncate'
writer.flush()
writer.close()
assert (root / 'open_write.txt').read_text() == 'alpha\nβ', 'text writes are committed'

# === Text append ===
append_writer = open(root / 'open_write.txt', 'a')
assert append_writer.write('!') == 1, 'append text returns character count'
append_writer.close()
assert (root / 'open_write.txt').read_text() == 'alpha\nβ!', 'text append extends file'

new_append_writer = open(root / 'open_new_append.txt', 'a')
assert new_append_writer.write('created') == 7, 'append creates missing file'
new_append_writer.close()
assert (root / 'open_new_append.txt').read_text() == 'created', 'append-created file readable'

# === Binary write and append ===
binary_writer = open(root / 'open_bytes.bin', 'wb')
assert str(type(binary_writer)) == "<class '_io.BufferedWriter'>", 'wb open returns BufferedWriter'
assert binary_writer.write(b'\x10\x11') == 2, 'binary write returns byte count'
assert binary_writer.write(b'\x12') == 1, 'second binary write appends'
binary_writer.close()
assert (root / 'open_bytes.bin').read_bytes() == b'\x10\x11\x12', 'binary writes are committed'

binary_append = open(root / 'open_bytes.bin', 'ab')
assert binary_append.write(b'\x13') == 1, 'binary append returns byte count'
binary_append.close()
assert (root / 'open_bytes.bin').read_bytes() == b'\x10\x11\x12\x13', 'binary append extends file'

# === Identity comparison: a file is equal to itself but not to a distinct handle ===
f = open(root / 'hello.txt')
assert f == f, 'file is equal to itself'
g = open(root / 'hello.txt')
assert f != g, 'two distinct handles to the same path are not equal'
f.close()
g.close()

# === '+' modes rejected on Monty (CPython accepts them; Monty's wrapper lacks
# read-position tracking so they would silently destroy data on first write) ===
if is_monty:
    for mode in ('r+', 'rb+', 'r+b', 'w+', 'wb+', 'a+', 'ab+'):
        try:
            open(root / 'open_bytes.bin', mode)
            assert False, f'expected ValueError for + mode {mode!r}'
        except ValueError as exc:
            assert str(exc) == "update modes ('+') are not yet supported", (
                f'unexpected + mode rejection for {mode!r}: {exc}'
            )

# === Keyword arguments ===
keyword_file = open(file=root / 'hello.txt', mode='r', encoding='utf-8')
assert keyword_file.read() == 'hello world\n', 'open accepts file/mode/encoding keywords'
keyword_file.close()

# === bytes path accepted (matches CPython os.fsdecode semantics) ===
hello_bytes = str(root / 'hello.txt').encode('utf-8')
bytes_path_file = open(hello_bytes)
assert bytes_path_file.read() == 'hello world\n', 'open accepts bytes paths via UTF-8 decode'
bytes_path_file.close()

# === All eight positional args accepted at CPython defaults ===
# Monty only honors `file` and `mode`; the other six must be at their CPython
# defaults (encoding='utf-8' is also accepted as a documented no-op since
# Monty already uses UTF-8).
positional = open(root / 'hello.txt', 'r', -1, 'utf-8', None, None, True, None)
assert positional.read() == 'hello world\n', 'open accepts default positional args + utf-8 encoding'
positional.close()

# closefd and opener also accepted as kwargs at their defaults
kw_closefd = open(root / 'hello.txt', closefd=True, opener=None)
kw_closefd.close()

# Non-default values for ignored kwargs are rejected on Monty (CPython
# silently honors them).
if is_monty:
    for kwarg_name, kwarg_value in (
        ('buffering', 0),
        ('encoding', 'latin-1'),
        ('errors', 'strict'),
        ('newline', ''),
        ('closefd', False),
    ):
        try:
            open(root / 'hello.txt', **{kwarg_name: kwarg_value})
            assert False, f'expected non-default {kwarg_name}={kwarg_value!r} to fail'
        except TypeError as exc:
            assert str(exc) == f"'{kwarg_name}' argument is not yet supported", (
                f'unexpected message for {kwarg_name}={kwarg_value!r}: {exc}'
            )

# === Open-time truncation / creation (CPython truncates/creates on open) ===
# w truncates an existing file immediately, before (and even without) any write
(root / 'open_trunc.txt').write_text('previous contents')
trunc = open(root / 'open_trunc.txt', 'w')
assert (root / 'open_trunc.txt').read_text() == '', 'open(w) truncates immediately, before any write'
trunc.close()
assert (root / 'open_trunc.txt').read_text() == '', 'file stays empty after closing an unused w handle'

# w creates a missing file immediately, even with no write
opened_w = open(root / 'open_created_w.txt', 'w')
opened_w.close()
assert (root / 'open_created_w.txt').read_text() == '', 'open(w) creates the file immediately'

# a creates a missing file immediately, even with no write
opened_a = open(root / 'open_created_a.txt', 'a')
opened_a.close()
assert (root / 'open_created_a.txt').read_text() == '', 'open(a) creates the file immediately'

# a must NOT truncate existing content on open
(root / 'open_keep_a.txt').write_text('keep me')
keep = open(root / 'open_keep_a.txt', 'a')
assert (root / 'open_keep_a.txt').read_text() == 'keep me', 'open(a) does not truncate existing content'
keep.write('!')
keep.close()
assert (root / 'open_keep_a.txt').read_text() == 'keep me!', 'append writes after existing content'

# binary w truncates on open too
(root / 'open_trunc.bin').write_bytes(b'\xff\xfe')
btrunc = open(root / 'open_trunc.bin', 'wb')
assert (root / 'open_trunc.bin').read_bytes() == b'', 'open(wb) truncates immediately'
btrunc.close()

# === Open-time existence checks for read modes ===
# r on a missing file raises FileNotFoundError at open time (not on first read)
try:
    open(root / 'open_missing.txt', 'r')
    assert False, 'expected FileNotFoundError opening a missing file for read'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/open_missing.txt'", (
            f'unexpected missing-file message: {exc}'
        )
    elif not is_windows:
        assert str(exc).startswith("[Errno 2] No such file or directory: '"), f'exc message: {exc}'

# opening a directory for read raises IsADirectoryError at open time
try:
    open(root, 'r')
    assert False, 'expected IsADirectoryError opening a directory for read'
except IsADirectoryError as exc:
    if is_monty:
        assert str(exc) == "[Errno 21] Is a directory: '/mnt'", f'unexpected is-a-directory message: {exc}'
    elif not is_windows:
        assert str(exc).startswith('[Errno 21] Is a directory: '), f'exc message: {exc}'

# === Operation errors ===
try:
    text_file.read()
    assert False, 'expected read after close to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.', f'unexpected closed-file message: {exc}'

# write() to a closed file must not leak its (heap-allocated) data argument
closed_writer = open(root / 'open_closed.txt', 'w')
closed_writer.close()
try:
    closed_writer.write('payload' + str(1))
    assert False, 'expected write after close to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.', f'unexpected closed-write message: {exc}'

# an invalid ignored-kwarg type must not leak the file/mode arguments
try:
    open(root / 'hello.txt', encoding=123)
    assert False, 'expected non-str encoding to fail'
except TypeError as exc:
    assert str(exc) == "open() argument 'encoding' must be str or None, not int", (
        f'unexpected encoding type message: {exc}'
    )

try:
    open(root / 'hello.txt', 'r').write('x')
    assert False, 'expected writing to read-only file to fail'
except OSError as exc:
    assert str(exc) == 'not writable', f'unexpected not-writable message: {exc}'
    # Mode-violation errors must surface as io.UnsupportedOperation, not bare
    # OSError. CPython exposes the class as `io.UnsupportedOperation` whose
    # `__name__` is the bare `UnsupportedOperation`; Monty uses the qualified
    # `io.UnsupportedOperation` as its single type identifier.
    expected_name = 'io.UnsupportedOperation' if is_monty else 'UnsupportedOperation'
    assert type(exc).__name__ == expected_name, f'expected {expected_name}, got {type(exc).__name__}'

try:
    open(root / 'hello.txt', 'rb').write(b'x')
    assert False, 'expected writing to rb file to fail'
except OSError as exc:
    assert str(exc) == 'write', f'unexpected binary not-writable message: {exc}'

try:
    open(root / 'hello.txt', 'w').read()
    assert False, 'expected reading from write-only file to fail'
except OSError as exc:
    assert str(exc) == 'not readable', f'unexpected not-readable message: {exc}'

# io.UnsupportedOperation also inherits from ValueError in CPython; Monty
# matches that behaviour so `except ValueError:` also catches mode violations.
try:
    open(root / 'hello.txt', 'w').read()
    assert False, 'expected reading from write-only file to fail'
except ValueError as exc:
    assert str(exc) == 'not readable', f'unexpected not-readable message: {exc}'

try:
    open(root / 'bad.txt', 'w').write(b'bytes')
    assert False, 'expected bytes write to text file to fail'
except TypeError as exc:
    assert str(exc) == 'write() argument must be str, not bytes', f'unexpected text write type message: {exc}'

try:
    open(root / 'bad.bin', 'wb').write('text')
    assert False, 'expected str write to binary file to fail'
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'str'", f'unexpected binary write type message: {exc}'

try:
    open(root / 'bad.txt', 'rw')
    assert False, 'expected invalid mode to fail'
except ValueError as exc:
    assert str(exc) == 'must have exactly one of create/read/write/append mode', (
        f'unexpected invalid mode message: {exc}'
    )

# === Empty mode and unknown-character mode parse errors ===
try:
    open(root / 'hello.txt', '')
    assert False, 'expected empty mode to fail'
except ValueError as exc:
    assert str(exc) == 'Must have exactly one of create/read/write/append mode and at most one plus', (
        f'unexpected empty mode message: {exc}'
    )

try:
    open(root / 'hello.txt', 'z')
    assert False, 'expected unknown mode character to fail'
except ValueError as exc:
    assert str(exc) == "invalid mode: 'z'", f'unexpected unknown mode message: {exc}'

# === Path.open() — same OsCall as builtin open() with `self` as the file ===
# Mode/kwarg validation, open-time effects, returned wrapper types, and
# context-manager semantics are all shared with `open()` above. The tests
# here focus on what's specific to going through Path: that the implicit
# `self` is used as the file argument, that positional and keyword `mode`
# both work, and that validation still fires when called via Path.
#
# Each test uses its own dedicated file because earlier tests in this file
# truncate `hello.txt` (`open(..., 'w').read()` truncates before raising).
(root / 'path_open_text.txt').write_text('hello via Path.open\n')
(root / 'path_open_bytes.bin').write_bytes(b'\x10\x20\x30')

path_read = (root / 'path_open_text.txt').open()
assert path_read.read() == 'hello via Path.open\n', 'Path.open() default-mode reads the file'
path_read.close()

# Positional mode reaches the same wrapper as `open(..., 'rb')`.
binary_via_path = (root / 'path_open_bytes.bin').open('rb')
assert str(type(binary_via_path)) == "<class '_io.BufferedReader'>", "Path.open('rb') returns BufferedReader"
assert binary_via_path.read() == b'\x10\x20\x30', "Path.open('rb') reads bytes"
binary_via_path.close()

# Keyword-only mode works too (no positional arg).
kw_mode = (root / 'path_open_text.txt').open(mode='r')
assert kw_mode.read() == 'hello via Path.open\n', 'Path.open(mode=...) reads the file'
kw_mode.close()

# `encoding='utf-8'` accepted as the documented no-op (Monty always uses UTF-8).
enc = (root / 'path_open_text.txt').open('r', encoding='utf-8')
assert enc.read() == 'hello via Path.open\n', "Path.open('r', encoding='utf-8') accepted"
enc.close()

# Context manager works through Path.open() — closes on exit on both paths.
with (root / 'path_open_text.txt').open() as f:
    assert f.read() == 'hello via Path.open\n', 'Path.open() works as context manager'
assert f.closed, 'context-manager exit closes the Path.open() file'

# Write through Path.open() lands the same content as a direct open().
(root / 'path_open_write.txt').open('w').write('written via Path.open\n')
assert (root / 'path_open_write.txt').read_text() == 'written via Path.open\n', "Path.open('w') write committed"

# Mode validation is shared — Monty rejects '+' modes; CPython would accept
# them, so this is monty-only.
if is_monty:
    try:
        (root / 'path_open_text.txt').open('r+')
        assert False, "expected ValueError for Path.open('r+')"
    except ValueError as exc:
        assert str(exc) == "update modes ('+') are not yet supported", f'unexpected Path.open r+ rejection: {exc}'

    try:
        (root / 'path_open_text.txt').open(buffering=0)
        assert False, 'expected TypeError for Path.open(buffering=0)'
    except TypeError as exc:
        assert str(exc) == "'buffering' argument is not yet supported", (
            f'unexpected Path.open buffering rejection: {exc}'
        )

# Open-time existence check still fires when called via Path.open().
try:
    (root / 'path_open_missing.txt').open()
    assert False, 'expected FileNotFoundError opening a missing file via Path.open'
except FileNotFoundError as exc:
    assert str(exc).startswith("[Errno 2] No such file or directory: '")
