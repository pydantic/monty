# `print()`

Output always goes to the host via a print callback (`vm.print_writer`).
The host decides where it ends up — there is no real `sys.stdout`
underneath (see [sys.md](sys.md)).

## Supported keyword arguments

- `sep=...` — separator between arguments. `None` falls back to a single
  space. Must be a `str` or `None`; otherwise `TypeError`.
- `end=...` — appended after the last argument. `None` falls back to `"\n"`.
  Must be a `str` or `None`; otherwise `TypeError`.

## Rejected / ignored

- `file=...` — explicitly rejected with `TypeError: "print() 'file'
  argument is not supported"`. Code that does `print(..., file=sys.stderr)`
  will not work; `sys.stderr` is an opaque marker (see [sys.md](sys.md)).
- `flush=...` — silently accepted but ignored. Monty does not buffer print
  output; the host receives each call immediately.
- Any other keyword raises `TypeError: ... unexpected keyword argument`.

## Behaviour

- Each positional argument is converted via `py_str` (equivalent to
  `str(x)`) before being written.
- The host callback receives the formatted string for each chunk; there
  is no atomicity guarantee across multiple `print()` calls if the host
  interleaves with other output.
