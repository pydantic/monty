# Standard library modules

Monty ships a fixed set of built-in stdlib modules. `import` of anything
else raises `ModuleNotFoundError` — there is no `sys.path`, no site-packages,
and no way for sandboxed code to load additional modules.

## Modules available

| Module     | See                                  |
| ---------- | ------------------------------------ |
| `asyncio`  | [asyncio.md](asyncio.md)             |
| `datetime` | [datetime.md](datetime.md)           |
| `json`     | [json.md](json.md)                   |
| `math`     | [math.md](math.md)                   |
| `os`       | [os.md](os.md)                       |
| `pathlib`  | [pathlib.md](pathlib.md)             |
| `re`       | [re.md](re.md)                       |
| `sys`      | [sys.md](sys.md)                     |
| `typing`   | [typing.md](typing.md)               |

A `gc` module exposing `collect()` / `enable()` / `disable()` is compiled
in only under the `test-hooks` Cargo feature for use by Monty's own test
suite; production sandboxes never see it.

## Notable modules NOT available

Common modules that are *not* importable in Monty (non-exhaustive):
`abc`, `argparse`, `array`, `base64`, `bisect`, `collections` (no
`defaultdict`, `Counter`, `OrderedDict`, `deque`; `namedtuple` is exposed
as a builtin, not via `collections`), `contextlib`, `copy`, `csv`,
`ctypes`, `dataclasses` (the `@dataclass` decorator is built in; the
module is not importable), `decimal`, `enum`, `fractions`, `functools`,
`hashlib`, `heapq`, `hmac`, `http`, `inspect`, `io`, `itertools`,
`logging`, `multiprocessing`, `operator`, `pickle`, `queue`, `random`,
`socket`, `string`, `struct`, `subprocess`, `tempfile`, `threading`,
`time`, `traceback`, `unittest`, `urllib`, `uuid`, `warnings`, `weakref`,
`zipfile`, `zlib`.

Many of these are deliberately excluded (`socket`, `subprocess`,
`multiprocessing`, `threading`, `ctypes`) because they would breach the
sandbox. Others (`itertools`, `functools`, `collections`, `enum`) are
simply unimplemented; they may appear over time.
