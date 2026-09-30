# call-external
# `os.urandom(size)` asks the host for `size` bytes.
import os

assert os.urandom(0) == b''
assert type(os.urandom(1)) is bytes
assert len(os.urandom(16)) == 16
assert len(os.urandom(True)) == 1
