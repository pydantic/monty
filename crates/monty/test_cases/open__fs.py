# mount-fs

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

# === Operation errors ===
try:
    text_file.read()
    assert False, 'expected read after close to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.', f'unexpected closed-file message: {exc}'

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
