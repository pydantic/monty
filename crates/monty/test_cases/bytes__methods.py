# === bytes.decode() ===
assert b'hello'.decode() == 'hello', 'decode default utf-8'
assert b'hello'.decode('utf-8') == 'hello', 'decode explicit utf-8'
assert b'hello'.decode('utf8') == 'hello', 'decode utf8 variant'
assert b'hello'.decode('UTF-8') == 'hello', 'decode uppercase UTF-8'
assert b''.decode() == '', 'decode empty bytes'

# Non-ASCII UTF-8
assert b'\xc3\xa9'.decode() == '\xe9', 'decode utf-8 e-acute'
assert b'\xe4\xb8\xad'.decode() == '\u4e2d', 'decode utf-8 CJK character'

# === bytes.count() ===
assert b'hello'.count(b'l') == 2, 'count single char'
assert b'hello'.count(b'll') == 1, 'count subsequence'
assert b'hello'.count(b'x') == 0, 'count not found'
assert b'aaa'.count(b'aa') == 1, 'count non-overlapping'
assert b''.count(b'x') == 0, 'count in empty bytes'
assert b'hello'.count(b'') == 6, 'count empty subsequence'

# count with start/end
assert b'abcabc'.count(b'ab', 1) == 1, 'count with start'
assert b'abcabc'.count(b'ab', 0, 3) == 1, 'count with start and end'

# === bytes.find() ===
assert b'hello'.find(b'e') == 1, 'find single char'
assert b'hello'.find(b'll') == 2, 'find subsequence'
assert b'hello'.find(b'x') == -1, 'find not found'
assert b'hello'.find(b'') == 0, 'find empty subsequence'
assert b''.find(b'x') == -1, 'find in empty bytes'

# find with start/end
assert b'hello'.find(b'l', 3) == 3, 'find with start'
assert b'hello'.find(b'l', 0, 2) == -1, 'find with end before match'

# === bytes.index() ===
assert b'hello'.index(b'e') == 1, 'index single char'
assert b'hello'.index(b'll') == 2, 'index subsequence'
assert b'hello'.index(b'') == 0, 'index empty subsequence'

# === bytes.startswith() ===
assert b'hello'.startswith(b'he'), 'startswith true'
assert not b'hello'.startswith(b'lo'), 'startswith false'
assert b'hello'.startswith(b''), 'startswith empty'
assert b''.startswith(b''), 'empty startswith empty'
assert not b''.startswith(b'x'), 'empty startswith non-empty'

# startswith with start/end
assert b'abcdef'.startswith(b'bc', 1), 'startswith with start'
assert b'abcdef'.startswith(b'bc', 1, 3), 'startswith with start and end'
assert not b'abcdef'.startswith(b'bc', 2), 'startswith with start past match'
assert not b'abcdef'.startswith(b'abc', 0, 2), 'startswith with end before match ends'

# === bytes.endswith() ===
assert b'hello'.endswith(b'lo'), 'endswith true'
assert not b'hello'.endswith(b'he'), 'endswith false'
assert b'hello'.endswith(b''), 'endswith empty'
assert b''.endswith(b''), 'empty endswith empty'
assert not b''.endswith(b'x'), 'empty endswith non-empty'

# endswith with start/end
assert b'abcdef'.endswith(b'de', 0, 5), 'endswith with end'
assert b'abcdef'.endswith(b'cd', 1, 4), 'endswith with start and end'
assert not b'abcdef'.endswith(b'de', 0, 4), 'endswith before suffix'

# === Edge case: start > end (should not panic, treat as empty slice) ===
assert b'hello'.find(b'e', 5, 2) == -1, 'find with start > end returns -1'
assert b'hello'.count(b'l', 5, 2) == 0, 'count with start > end returns 0'
assert not b'hello'.startswith(b'h', 5, 2), 'startswith with start > end is false'
assert not b'hello'.endswith(b'o', 5, 2), 'endswith with start > end is false'
