# Exercise long scans, ordinary line endings, and Python's Unicode line boundaries.

ascii_line = 'a' * 80
unicode_line = 'αβγé😀' * 16
samples = [
    ('a' * 32768, 1),
    ((ascii_line + '\n' + 'b' * 80 + '\r\n') * 128, 256),
    ((unicode_line + '\r\n') * 128, 128),
    ((unicode_line + '\x85' + ascii_line + '\u2028' + unicode_line + '\u2029') * 64, 192),
]

for text, expected_lines in samples:
    for _ in range(20):
        lines = text.splitlines()
    assert len(lines) == expected_lines
    for _ in range(20):
        lines = text.splitlines(True)
    assert len(lines) == expected_lines
    assert ''.join(lines) == text
