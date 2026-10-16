# A recursive glob over the mounted scan tree: every directory is listed.
# `overlay` also runs it over an overlay mount.
# bench: mount overlay

from pathlib import Path

assert len(list(Path(DATA).glob('**/*.rs'))) == 1_020
