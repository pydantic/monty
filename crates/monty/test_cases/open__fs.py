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
text_file.close()
assert text_file.closed == True, 'close sets closed'

# === Binary read ===
binary_file = open(root / 'data.bin', 'rb')
assert str(type(binary_file)) == "<class '_io.BufferedReader'>", 'rb open returns BufferedReader'
assert binary_file.mode == 'rb', 'binary mode is preserved'
assert binary_file.read() == b'\x00\x01\x02\x03', 'binary read returns bytes'
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

binary_random = open(root / 'open_bytes.bin', 'r+b')
assert str(type(binary_random)) == "<class '_io.BufferedRandom'>", 'r+b open returns BufferedRandom'
assert binary_random.read() == b'\x10\x11\x12\x13', 'binary random can read'
binary_random.close()

# === Keyword arguments ===
keyword_file = open(file=root / 'hello.txt', mode='r', encoding='utf-8')
assert keyword_file.read() == 'hello world\n', 'open accepts file/mode/encoding keywords'
keyword_file.close()

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

# r+ on a missing file likewise raises at open time
try:
    open(root / 'open_missing_rplus.txt', 'r+')
    assert False, 'expected FileNotFoundError opening a missing file for r+'
except FileNotFoundError as exc:
    if is_monty:
        assert str(exc) == "[Errno 2] No such file or directory: '/mnt/open_missing_rplus.txt'", (
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
