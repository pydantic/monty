# `os.path` is `posixpath` on every host: the sandbox path model is POSIX, so
# upstream's `sys.platform` switch to `ntpath` must never apply.
from posixpath import *
