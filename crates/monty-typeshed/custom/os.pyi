from abc import ABC, abstractmethod
from typing import AnyStr, Callable, Protocol, TypeAlias, TypeVar, final, overload, runtime_checkable

from _typeshed import AnyStr_co, structseq

_T = TypeVar('_T')
environ: dict[str, str]

# POSIX path constants — the sandbox path model is POSIX on every host.
sep: str
altsep: str | None
extsep: str
curdir: str
pardir: str
linesep: str
name: str
devnull: str

@overload
def getenv(key: str) -> str | None: ...
@overload
def getenv(key: str, default: _T) -> str | _T: ...
def listdir(path: str | PathLike[str] | None = None) -> list[str]: ...
def stat(path: str | PathLike[str], *, dir_fd: int | None = None, follow_symlinks: bool = True) -> stat_result: ...
def mkdir(path: str | PathLike[str], mode: int = 0o777, *, dir_fd: int | None = None) -> None: ...
def makedirs(name: str | PathLike[str], mode: int = 0o777, exist_ok: bool = False) -> None: ...
def remove(path: str | PathLike[str], *, dir_fd: int | None = None) -> None: ...
def unlink(path: str | PathLike[str], *, dir_fd: int | None = None) -> None: ...
def rmdir(path: str | PathLike[str], *, dir_fd: int | None = None) -> None: ...
def rename(
    src: str | PathLike[str], dst: str | PathLike[str], *, src_dir_fd: int | None = None, dst_dir_fd: int | None = None
) -> None: ...
def replace(
    src: str | PathLike[str], dst: str | PathLike[str], *, src_dir_fd: int | None = None, dst_dir_fd: int | None = None
) -> None: ...
@overload
def fspath(path: str) -> str: ...
@overload
def fspath(path: bytes) -> bytes: ...
@overload
def fspath(path: PathLike[AnyStr]) -> AnyStr: ...

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
