from typing import Any, Final, Literal, TextIO, final, type_check_only

from _typeshed import MaybeNone, structseq
from typing_extensions import TypeAlias

# stdin: TextIO | MaybeNone
stdout: TextIO | MaybeNone
stderr: TextIO | MaybeNone

argv: list[str]

version: str
hexversion: int
api_version: int
copyright: str
builtin_module_names: tuple[str, ...]

flags: _flags

maxsize: int
maxunicode: int
byteorder: Literal['little', 'big']
float_repr_style: Literal['short', 'legacy']

executable: str
prefix: str
exec_prefix: str
base_prefix: str
base_exec_prefix: str
platlibdir: str
abiflags: str
dont_write_bytecode: bool
pycache_prefix: str | None

# Type alias used as a mixin for structseq classes that cannot be instantiated at runtime
# This can't be represented in the type system, so we just use `structseq[Any]`
_UninstantiableStructseq: TypeAlias = structseq[Any]
_ReleaseLevel: TypeAlias = Literal['alpha', 'beta', 'candidate', 'final']

@final
@type_check_only
class _version_info(_UninstantiableStructseq, tuple[int, int, int, _ReleaseLevel, int]):
    __match_args__: Final = ('major', 'minor', 'micro', 'releaselevel', 'serial')

    @property
    def major(self) -> int: ...
    @property
    def minor(self) -> int: ...
    @property
    def micro(self) -> int: ...
    @property
    def releaselevel(self) -> _ReleaseLevel: ...
    @property
    def serial(self) -> int: ...

version_info: _version_info

@final
@type_check_only
class _float_info(structseq[float], tuple[float, int, int, float, int, int, int, int, float, int, int]):
    __match_args__: Final = (
        'max',
        'max_exp',
        'max_10_exp',
        'min',
        'min_exp',
        'min_10_exp',
        'dig',
        'mant_dig',
        'epsilon',
        'radix',
        'rounds',
    )

    @property
    def max(self) -> float: ...
    @property
    def max_exp(self) -> int: ...
    @property
    def max_10_exp(self) -> int: ...
    @property
    def min(self) -> float: ...
    @property
    def min_exp(self) -> int: ...
    @property
    def min_10_exp(self) -> int: ...
    @property
    def dig(self) -> int: ...
    @property
    def mant_dig(self) -> int: ...
    @property
    def epsilon(self) -> float: ...
    @property
    def radix(self) -> int: ...
    @property
    def rounds(self) -> int: ...

float_info: _float_info

@final
@type_check_only
class _flags(
    _UninstantiableStructseq,
    tuple[int, int, int, int, int, int, int, int, int, int, int, int, int, bool, int, int, bool, int],
):
    __match_args__: Final = (
        'debug',
        'inspect',
        'interactive',
        'optimize',
        'dont_write_bytecode',
        'no_user_site',
        'no_site',
        'ignore_environment',
        'verbose',
        'bytes_warning',
        'quiet',
        'hash_randomization',
        'isolated',
        'dev_mode',
        'utf8_mode',
        'warn_default_encoding',
        'safe_path',
        'int_max_str_digits',
    )

    @property
    def debug(self) -> int: ...
    @property
    def inspect(self) -> int: ...
    @property
    def interactive(self) -> int: ...
    @property
    def optimize(self) -> int: ...
    @property
    def dont_write_bytecode(self) -> int: ...
    @property
    def no_user_site(self) -> int: ...
    @property
    def no_site(self) -> int: ...
    @property
    def ignore_environment(self) -> int: ...
    @property
    def verbose(self) -> int: ...
    @property
    def bytes_warning(self) -> int: ...
    @property
    def quiet(self) -> int: ...
    @property
    def hash_randomization(self) -> int: ...
    @property
    def isolated(self) -> int: ...
    @property
    def dev_mode(self) -> bool: ...
    @property
    def utf8_mode(self) -> int: ...
    @property
    def warn_default_encoding(self) -> int: ...
    @property
    def safe_path(self) -> bool: ...
    @property
    def int_max_str_digits(self) -> int: ...
