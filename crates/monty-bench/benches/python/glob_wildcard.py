# /// monty
# [bench]
# mount = true
# ///
# A glob whose wildcards prune: only `src` directories below the top are listed.

from pathlib import Path

assert len(list(Path(DATA).glob('*/src/*.rs'))) == 20
