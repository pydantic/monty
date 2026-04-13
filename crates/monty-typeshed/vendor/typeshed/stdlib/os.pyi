from abc import ABC, abstractmethod
from typing import Callable, Protocol, TypeAlias, TypeVar, final, overload, runtime_checkable

from _typeshed import AnyStr_co, structseq

_T = TypeVar('_T')
environ: dict[str, str]

@overload
def getenv(key: str) -> str | None: ...
@overload
def getenv(key: str, default: _T) -> str | _T: ...
@final
class stat_result(structseq[float], tuple[int, int, int, int, int, int, int, float, float, float]):
    # The constructor of this class takes an iterable of variable length (though it must be at least 10).
    #
    # However, this class behaves like a tuple of 10 elements,
    # no matter how long the iterable supplied to the constructor is.
    # https://github.com/python/typeshed/pull/6560#discussion_r767162532
    #
    # The 10 elements always present are st_mode, st_ino, st_dev, st_nlink,
    # st_uid, st_gid, st_size, st_atime, st_mtime, st_ctime.
    #
    # More items may be added at the end by some implementations.

    @property
    def st_mode(self) -> int:
        """protection bits"""
        ...

    @property
    def st_ino(self) -> int:
        """inode"""
        ...

    @property
    def st_dev(self) -> int:
        """device"""
        ...

    @property
    def st_nlink(self) -> int:
        """number of hard links"""
        ...

    @property
    def st_uid(self) -> int:
        """user ID of owner"""
        ...

    @property
    def st_gid(self) -> int:
        """group ID of owner"""
        ...

    @property
    def st_size(self) -> int:
        """total size, in bytes"""
        ...

    @property
    def st_atime(self) -> float:
        """time of last access"""
        ...

    @property
    def st_mtime(self) -> float:
        """time of last modification"""
        ...

    @property
    def st_ctime(self) -> float:
        """time of last change"""
        ...

# (Samuel) PathLike is included here because it's used by pathlib

# mypy and pyright object to this being both ABC and Protocol.
# At runtime it inherits from ABC and is not a Protocol, but it will be
# on the allowlist for use as a Protocol starting in 3.14.
@runtime_checkable
class PathLike(ABC, Protocol[AnyStr_co]):  # type: ignore[misc]  # pyright: ignore[reportGeneralTypeIssues]
    __slots__ = ()
    @abstractmethod
    def __fspath__(self) -> AnyStr_co: ...

_Opener: TypeAlias = Callable[[str, int], int]
