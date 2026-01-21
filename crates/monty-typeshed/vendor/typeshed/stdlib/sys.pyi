from typing import Any, Final, Literal, TextIO, final, type_check_only

from _typeshed import MaybeNone, structseq
from typing_extensions import TypeAlias

# stdin: TextIO | MaybeNone
stdout: TextIO | MaybeNone
stderr: TextIO | MaybeNone

version: str

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
