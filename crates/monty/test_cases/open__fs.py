# mount-fs
# skip-cpython-windows — CPython on Windows defaults text I/O to cp1252 which can't encode β;
# Windows-specific coverage lives in open__fs_windows.py.
import sys

is_monty = sys.platform == 'monty'
is_windows = sys.platform == 'win32'

# === Text read ===
text_file = open(root / 'hello.txt')
assert str(type(text_file)) == "<class '_io.TextIOWrapper'>"
assert text_file.mode == 'r'
assert text_file.readable() == True
assert text_file.writable() == False
assert text_file.read() == 'hello world\n'
# Second sequential read should be empty (CPython EOF semantics)
assert text_file.read() == ''
assert text_file.read() == ''
# Both CPython and Monty report readable files as seekable now that Monty
# implements seek()/tell() via on-demand buffering.
assert text_file.seekable() == True
text_file.close()
assert text_file.closed == True

# === Binary read ===
binary_file = open(root / 'data.bin', 'rb')
assert str(type(binary_file)) == "<class '_io.BufferedReader'>"
assert binary_file.mode == 'rb'
assert binary_file.read() == b'\x00\x01\x02\x03'
assert binary_file.read() == b''
binary_file.close()

# === Text write ===
writer = open(root / 'open_write.txt', 'w')
assert str(type(writer)) == "<class '_io.TextIOWrapper'>"
assert writer.readable() == False
assert writer.writable() == True
assert writer.write('alpha') == 5
assert writer.write('\nβ') == 2
writer.flush()
writer.close()
assert (root / 'open_write.txt').read_text() == 'alpha\nβ'

# === Text append ===
append_writer = open(root / 'open_write.txt', 'a')
assert append_writer.write('!') == 1
append_writer.close()
assert (root / 'open_write.txt').read_text() == 'alpha\nβ!'

new_append_writer = open(root / 'open_new_append.txt', 'a')
assert new_append_writer.write('created') == 7
new_append_writer.close()
assert (root / 'open_new_append.txt').read_text() == 'created'

# === Binary write and append ===
binary_writer = open(root / 'open_bytes.bin', 'wb')
assert str(type(binary_writer)) == "<class '_io.BufferedWriter'>"
assert binary_writer.write(b'\x10\x11') == 2
assert binary_writer.write(b'\x12') == 1
binary_writer.close()
assert (root / 'open_bytes.bin').read_bytes() == b'\x10\x11\x12'

binary_append = open(root / 'open_bytes.bin', 'ab')
assert binary_append.write(b'\x13') == 1
binary_append.close()
assert (root / 'open_bytes.bin').read_bytes() == b'\x10\x11\x12\x13'

# === Identity comparison: a file is equal to itself but not to a distinct handle ===
f = open(root / 'hello.txt')
assert f == f
g = open(root / 'hello.txt')
assert f != g
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
assert keyword_file.read() == 'hello world\n'
keyword_file.close()

# === bytes path accepted (matches CPython os.fsdecode semantics) ===
hello_bytes = str(root / 'hello.txt').encode('utf-8')
bytes_path_file = open(hello_bytes)
assert bytes_path_file.read() == 'hello world\n'
bytes_path_file.close()

# === All eight positional args accepted at CPython defaults ===
# Monty only honors `file` and `mode`; the other six must be at their CPython
# defaults (encoding='utf-8' is also accepted as a documented no-op since
# Monty already uses UTF-8).
positional = open(root / 'hello.txt', 'r', -1, 'utf-8', None, None, True, None)
assert positional.read() == 'hello world\n'
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
assert (root / 'open_trunc.txt').read_text() == ''
trunc.close()
assert (root / 'open_trunc.txt').read_text() == ''

# w creates a missing file immediately, even with no write
opened_w = open(root / 'open_created_w.txt', 'w')
opened_w.close()
assert (root / 'open_created_w.txt').read_text() == ''

# a creates a missing file immediately, even with no write
opened_a = open(root / 'open_created_a.txt', 'a')
opened_a.close()
assert (root / 'open_created_a.txt').read_text() == ''

# a must NOT truncate existing content on open
(root / 'open_keep_a.txt').write_text('keep me')
keep = open(root / 'open_keep_a.txt', 'a')
assert (root / 'open_keep_a.txt').read_text() == 'keep me'
keep.write('!')
keep.close()
assert (root / 'open_keep_a.txt').read_text() == 'keep me!'

# binary w truncates on open too
(root / 'open_trunc.bin').write_bytes(b'\xff\xfe')
btrunc = open(root / 'open_trunc.bin', 'wb')
assert (root / 'open_trunc.bin').read_bytes() == b''
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

# === Wrong-type `mode` matches CPython's `_PyArg_BadArgument` wording ===
# Driven by `bad_arg_named` on `OpenArgs`. The `None`-vs-`NoneType` special
# case must apply (lone `None` reads as `"not None"`).
for bad, expected_type in ((42, 'int'), (None, 'None'), (b'r', 'bytes')):
    try:
        open(root / 'hello.txt', bad)
        assert False, f'open(mode={bad!r}) should error'
    except TypeError as exc:
        assert str(exc) == f"open() argument 'mode' must be str, not {expected_type}", (
            f'open(mode={bad!r}) wrong type: {exc}'
        )
    try:
        open(root / 'hello.txt', mode=bad)
        assert False, f'open(mode={bad!r} kwarg) should error'
    except TypeError as exc:
        assert str(exc) == f"open() argument 'mode' must be str, not {expected_type}", (
            f'open(mode={bad!r} kwarg) wrong type: {exc}'
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

# A `b`/`t` flag alone supplies no r/w/a action and must be rejected, not
# silently treated as a read.
try:
    open(root / 'hello.txt', 'b')
    assert False, 'expected action-less binary mode to fail'
except ValueError as exc:
    assert str(exc) == 'Must have exactly one of create/read/write/append mode and at most one plus', (
        f'unexpected action-less mode message: {exc}'
    )

try:
    open(root / 'hello.txt', 't')
    assert False, 'expected action-less text mode to fail'
except ValueError as exc:
    assert str(exc) == 'Must have exactly one of create/read/write/append mode and at most one plus', (
        f'unexpected action-less mode message: {exc}'
    )

# === Sized read ===
# Set up a multi-line text fixture for the rest of these tests.
(root / 'sized.txt').write_text('hello world')
sized = open(root / 'sized.txt')
assert sized.read(5) == 'hello'
assert sized.read(100) == ' world'
assert sized.read(1) == ''
assert sized.read(0) == ''
sized.close()

read_none = open(root / 'sized.txt')
assert read_none.read(None) == 'hello world'
read_none.close()

read_bool = open(root / 'sized.txt')
assert read_bool.read(True) == 'h'
assert read_bool.read(False) == ''
assert read_bool.read(1) == 'e'
read_bool.close()

# read(-1) and read() (with buffer loaded) return the rest
mixed = open(root / 'sized.txt')
assert mixed.read(5) == 'hello'
assert mixed.read(-1) == ' world'
mixed.close()

mixed2 = open(root / 'sized.txt')
assert mixed2.read(5) == 'hello'
assert mixed2.read() == ' world'
mixed2.close()

# read(0) without prior reads short-circuits without loading
zero = open(root / 'sized.txt')
assert zero.read(0) == ''
assert zero.read(5) == 'hello'
zero.close()

# Binary sized read
(root / 'sized.bin').write_bytes(b'\x10\x11\x12\x13\x14')
sized_b = open(root / 'sized.bin', 'rb')
assert sized_b.read(3) == b'\x10\x11\x12'
assert sized_b.read(10) == b'\x13\x14'
assert sized_b.read(1) == b''
sized_b.close()

# === readline / readlines / tell / seek ===
(root / 'lines.txt').write_text('first\nsecond\nthird')
lf = open(root / 'lines.txt')
assert lf.readline() == 'first\n'
assert lf.readline() == 'second\n'
assert lf.readline() == 'third'
assert lf.readline() == ''
lf.close()

# readline on an empty file
(root / 'empty_lines.txt').write_text('')
empty_lf = open(root / 'empty_lines.txt')
assert empty_lf.readline() == ''
empty_lf.close()

# readlines
all_lines = open(root / 'lines.txt')
assert all_lines.readlines() == ['first\n', 'second\n', 'third']
assert all_lines.readlines() == []
all_lines.close()

# Binary readline / readlines
(root / 'lines.bin').write_bytes(b'a\nb\nc')
bin_lf = open(root / 'lines.bin', 'rb')
assert bin_lf.readline() == b'a\n'
assert bin_lf.readlines() == [b'b\n', b'c']
bin_lf.close()

# tell()
t = open(root / 'sized.txt')
assert t.tell() == 0
t.read(5)
assert t.tell() == 5
t.read(2)
assert t.tell() == 7
t.close()

# seek()
s = open(root / 'sized.txt')
assert s.read(11) == 'hello world'
assert s.tell() == 11
assert s.seek(0) == 0
assert s.tell() == 0
assert s.read(5) == 'hello'
assert s.seek(0, 2) == 11
assert s.read(1) == ''
assert s.seek(6) == 6
assert s.read(5) == 'world'
s.close()

# seek() with no prior reads triggers the buffer load
fresh_seek = open(root / 'sized.txt')
assert fresh_seek.seek(0, 2) == 11
assert fresh_seek.tell() == 11
assert fresh_seek.read(1) == ''
fresh_seek.close()

# seek(offset, 1) — SEEK_CUR — adjusts position relative to current.
# CPython's TextIOWrapper rejects nonzero cur-relative seeks, so the
# text-mode case is restricted to seek(0, 1) (which is a no-op tell()).
cur_t = open(root / 'sized.txt')
assert cur_t.read(4) == 'hell'
assert cur_t.seek(0, 1) == 4
assert cur_t.read(2) == 'o '
cur_t.close()

# Binary SEEK_CUR supports nonzero offsets in both CPython and Monty.
cur_b = open(root / 'sized.bin', 'rb')
assert cur_b.read(2) == b'\x10\x11'
assert cur_b.seek(1, 1) == 3
assert cur_b.read(2) == b'\x13\x14'
assert cur_b.seek(-3, 1) == 2
assert cur_b.read(2) == b'\x12\x13'
cur_b.close()

# Negative seek raises OSError in binary mode (matches CPython's BufferedReader).
ns = open(root / 'sized.bin', 'rb')
ns.read(0)  # no-op
try:
    ns.seek(-1)
    assert False, 'expected OSError for negative seek'
except OSError as exc:
    assert str(exc) == '[Errno 22] Invalid argument', f'unexpected negative seek message: {exc}'
ns.close()

# Invalid whence
iw = open(root / 'sized.bin', 'rb')
iw.read(0)
try:
    iw.seek(0, 99)
    assert False, 'expected ValueError for invalid whence'
except ValueError as exc:
    assert str(exc) == 'whence value 99 unsupported', f'unexpected invalid whence message: {exc}'
try:
    iw.seek(0, -1)
    assert False, 'expected ValueError for negative whence'
except ValueError as exc:
    assert str(exc) == 'whence value -1 unsupported', f'unexpected negative whence message: {exc}'
try:
    iw.seek(0, 256)
    assert False, 'expected ValueError for large whence'
except ValueError as exc:
    assert str(exc) == 'whence value 256 unsupported', f'unexpected large whence message: {exc}'
iw.close()

# tell / seek on a closed file
closed_t = open(root / 'sized.txt')
closed_t.close()
try:
    closed_t.tell()
    assert False, 'expected tell on closed file to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.'
try:
    closed_t.seek(0)
    assert False, 'expected seek on closed file to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.'

# UTF-8 multi-byte handling: 'β' is 2 bytes, 1 char.
(root / 'utf8.txt').write_text('aβc')
utf = open(root / 'utf8.txt')
assert utf.read(2) == 'aβ'
# Monty's text-mode tell() is a char index (diverges from CPython's opaque
# byte cookie); see limitations/open.md.
if is_monty:
    assert utf.tell() == 2
assert utf.read(1) == 'c'
utf.close()

# tell/seek round-trip
rt = open(root / 'lines.txt')
rt.read(3)
captured = rt.tell()
assert captured == 3
rt.read(5)
assert rt.seek(captured) == 3
assert rt.read(2) == 'st'
rt.close()

# Mixed read() / readline()
mixed_rl = open(root / 'lines.txt')
assert mixed_rl.read(2) == 'fi'
assert mixed_rl.readline() == 'rst\n'
assert mixed_rl.read() == 'second\nthird'
mixed_rl.close()

# read on a 'w' file
try:
    open(root / 'open_write.txt', 'w').read(5)
    assert False, 'expected read on w to fail'
except OSError as exc:
    assert str(exc) == 'not readable'

# read(0) on a write-only file: the open/readable check still happens first
try:
    open(root / 'open_write.txt', 'w').read(0)
    assert False, 'expected read(0) on w to fail'
except OSError as exc:
    assert str(exc) == 'not readable'

# read on a closed write-only file should hit ensure_open before mode check
closed_w = open(root / 'open_write.txt', 'w')
closed_w.close()
try:
    closed_w.read(0)
    assert False, 'expected read(0) on closed file to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.'

# write-only files still expose logical tell/seek state.
write_pos = open(root / 'write_position.txt', 'w')
assert write_pos.seekable() == True
assert write_pos.tell() == 0
assert write_pos.write('abc') == 3
assert write_pos.tell() == 3
assert write_pos.seek(0) == 0
assert write_pos.tell() == 0
assert write_pos.seek(0, 2) == 3
write_pos.close()

write_pos_b = open(root / 'write_position.bin', 'wb')
assert write_pos_b.write(b'\x00\x01\x02\x03') == 4
assert write_pos_b.tell() == 4
write_pos_b.close()

# Regression: binary read after seek-past-end must not panic.
(root / 'past_end.bin').write_bytes(b'12345')
pe = open(root / 'past_end.bin', 'rb')
assert pe.seek(100) == 100
assert pe.tell() == 100
assert pe.read(5) == b''
assert pe.tell() == 100
assert pe.readline() == b''
assert pe.tell() == 100
assert pe.readlines() == []
assert pe.tell() == 100
assert pe.read() == b''
assert pe.tell() == 100
pe.close()

# Same regression in text mode.
(root / 'past_end.txt').write_text('hello')
pet = open(root / 'past_end.txt')
assert pet.seek(100) == 100
assert pet.read(5) == ''
assert pet.tell() == 100
assert pet.readline() == ''
assert pet.tell() == 100
assert pet.readlines() == []
assert pet.tell() == 100
assert pet.read() == ''
assert pet.tell() == 100
pet.close()

# read(N) with non-int arg raises TypeError (exact message diverges from CPython).
nt = open(root / 'sized.txt')
try:
    nt.read('5')
    assert False, 'expected TypeError for non-int read size'
except TypeError as exc:
    msg = str(exc)
    if is_monty:
        assert msg == "'str' object cannot be interpreted as an integer", f'unexpected message: {msg}'
    else:
        assert msg == "argument should be integer or None, not 'str'", f'unexpected CPython message: {msg}'
nt.close()

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
assert path_read.read() == 'hello via Path.open\n'
path_read.close()

# Positional mode reaches the same wrapper as `open(..., 'rb')`.
binary_via_path = (root / 'path_open_bytes.bin').open('rb')
assert str(type(binary_via_path)) == "<class '_io.BufferedReader'>"
assert binary_via_path.read() == b'\x10\x20\x30'
binary_via_path.close()

# Keyword-only mode works too (no positional arg).
kw_mode = (root / 'path_open_text.txt').open(mode='r')
assert kw_mode.read() == 'hello via Path.open\n'
kw_mode.close()

# `encoding='utf-8'` accepted as the documented no-op (Monty always uses UTF-8).
enc = (root / 'path_open_text.txt').open('r', encoding='utf-8')
assert enc.read() == 'hello via Path.open\n'
enc.close()

# Context manager works through Path.open() — closes on exit on both paths.
with (root / 'path_open_text.txt').open() as f:
    assert f.read() == 'hello via Path.open\n'
assert f.closed

# Write through Path.open() lands the same content as a direct open().
(root / 'path_open_write.txt').open('w').write('written via Path.open\n')
assert (root / 'path_open_write.txt').read_text() == 'written via Path.open\n'

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
