# A glob whose wildcards prune: only `src` directories below the top are listed.
# bench: mount

from pathlib import Path

assert len(list(Path(DATA).glob('*/src/*.rs'))) == 20
