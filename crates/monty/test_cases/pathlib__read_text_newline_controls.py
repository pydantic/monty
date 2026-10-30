# /// monty
# mount-fs = true
# ///
from pathlib import Path

raw = b'head\r\nbody\r'
(root / 'raw-newline-controls.txt').write_bytes(raw)
assert (root / 'raw-newline-controls.txt').read_bytes() == raw
with open(root / 'raw-newline-controls.txt') as file:
    assert file.read() == 'head\nbody\n'
with open(root / 'raw-newline-controls.txt', newline='') as file:
    assert file.read() == 'head\r\nbody\r'

(root / 'lf-only.txt').write_bytes(b'first\nsecond\n')
assert Path(root / 'lf-only.txt').read_text() == 'first\nsecond\n'

(root / 'empty.txt').write_bytes(b'')
assert Path(root / 'empty.txt').read_text() == ''
