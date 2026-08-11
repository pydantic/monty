# mount-fs
# Windows-safe companion to open__fs.py covering the non-ASCII text I/O paths.
#
# open__fs.py is skipped on Windows CPython because its default text encoding
# is cp1252, which cannot encode the Greek β (U+03B2) used in the write tests.
# This file exercises the same write/append paths but pins `encoding='utf-8'`
# on every text `open()` so it runs identically on POSIX and Windows CPython.
# `Path.read_text` is avoided because it has no encoding kwarg in Monty and
# would silently decode as cp1252 on Windows CPython.
#
# `\n` characters are also avoided in text-mode writes: Windows CPython's
# default universal-newline translation rewrites `\n` to `\r\n` on write,
# whereas Monty performs no newline translation (see limitations/open.md).
# Monty rejects `newline=''` as a non-default kwarg, so we can't opt out;
# the test data is shaped to keep both interpreters byte-identical.

# === Text write with explicit utf-8 encoding ===
writer = open(root / 'open_write.txt', 'w', encoding='utf-8')
assert str(type(writer)) == "<class '_io.TextIOWrapper'>"
assert writer.readable() == False
assert writer.writable() == True
assert writer.write('alpha') == 5
assert writer.write('β') == 1
writer.flush()
writer.close()

reader = open(root / 'open_write.txt', 'r', encoding='utf-8')
assert reader.read() == 'alphaβ'
reader.close()

# === Text append with explicit utf-8 encoding ===
append_writer = open(root / 'open_write.txt', 'a', encoding='utf-8')
assert append_writer.write('!') == 1
append_writer.close()

reader = open(root / 'open_write.txt', 'r', encoding='utf-8')
assert reader.read() == 'alphaβ!'
reader.close()

# === Bytes on disk match utf-8 encoding of β ===
# β encodes as 0xCE 0xB2 in UTF-8 — verifies the file really was written as UTF-8
# rather than the host's default text encoding.
assert (root / 'open_write.txt').read_bytes() == b'alpha\xce\xb2!'
