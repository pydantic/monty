from typing import Any, Final, Literal, TextIO, final, type_check_only

from _typeshed import MaybeNone, structseq
from typing_extensions import TypeAlias

# stdin: TextIO | MaybeNone
stdout: TextIO | MaybeNone
stderr: TextIO | MaybeNone

version: str
hexversion: int
api_version: int
copyright: str
builtin_module_names: tuple[str, ...]

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
