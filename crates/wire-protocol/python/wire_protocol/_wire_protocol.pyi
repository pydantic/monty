from typing import Any, Mapping, final

__version__: str

# =============================================================================
# Codec functions
# =============================================================================

def encode_parent_request(request: ParentRequest) -> bytes:
    """Encode a `ParentRequest` (client → sandbox) to raw protobuf bytes.

    No length prefix is added: WebSocket/HTTP already frame the message; a raw
    byte stream needs its own 4-byte little-endian length prefix.
    """

def decode_parent_request(data: bytes) -> ParentRequest:
    """Decode the bytes of a `ParentRequest` (server side). Raises `ValueError`
    on malformed or invalid input."""

def encode_child_event(event: ChildEvent) -> bytes:
    """Encode a `ChildEvent` (sandbox → client) to raw protobuf bytes."""

def decode_child_event(data: bytes) -> ChildEvent:
    """Decode the bytes of a `ChildEvent` (client side). Raises `ValueError`
    on malformed or invalid input."""

# Sandbox values are native Python objects
# (int/str/bytes/list/dict/datetime/PurePosixPath/dataclasses/...).
Value = Any

# =============================================================================
# Payload types
# =============================================================================

@final
class Mount:
    """A host directory to expose into the sandbox for one `Feed`.

    Pure data: unlike `pydantic_monty.MountDir` it does no filesystem
    validation, because `host_path` is on the *server*, not the client.
    """

    virtual_path: str
    host_path: str
    mode: str
    write_bytes_limit: int | None
    def __new__(
        cls,
        virtual_path: str,
        host_path: str,
        *,
        mode: str = 'overlay',
        write_bytes_limit: int | None = None,
    ) -> Mount: ...

@final
class StackFrame:
    """One frame of a `RaisedException` traceback."""

    filename: str
    line: int
    column: int
    end_line: int
    end_column: int
    function_name: str | None
    preview_line: str | None
    hide_caret: bool
    hide_frame_name: bool
    def __new__(
        cls,
        filename: str,
        line: int,
        column: int,
        end_line: int,
        end_column: int,
        *,
        function_name: str | None = None,
        preview_line: str | None = None,
        hide_caret: bool = False,
        hide_frame_name: bool = False,
    ) -> StackFrame: ...

@final
class RaisedException:
    """A raised Python exception crossing the wire: type, message, traceback."""

    exc_type: str
    message: str | None
    traceback: list[StackFrame]
    def __new__(
        cls,
        exc_type: str,
        message: str | None = None,
        traceback: list[StackFrame] | None = None,
    ) -> RaisedException: ...
    @classmethod
    def from_exception(cls, exc: BaseException) -> RaisedException:
        """Build from a caught Python exception (its type and `str()`; no
        traceback is captured)."""

    def as_exception(self) -> BaseException:
        """Reconstruct the native Python exception instance (e.g. a real
        `ValueError`) so a server can re-raise what the sandbox raised."""

@final
class ExtFunctionResult:
    """The outcome of a host-side function/OS call, for `ResumeCall` /
    `FutureResult`. Build via the classmethods."""

    @classmethod
    def returns(cls, value: Value) -> ExtFunctionResult:
        """The call returned `value`."""

    @classmethod
    def error(cls, exception: RaisedException) -> ExtFunctionResult:
        """The call raised `exception`."""

    @classmethod
    def future(cls, call_id: int) -> ExtFunctionResult:
        """The call is asynchronous; resolve `call_id` later via
        `ResumeFutures`."""

    @classmethod
    def not_found(cls, name: str) -> ExtFunctionResult:
        """No handler exists; the sandbox raises `NameError`."""

@final
class FutureResult:
    """A resolved future: a pending `call_id` and its result."""

    call_id: int
    result: ExtFunctionResult
    def __new__(cls, call_id: int, result: ExtFunctionResult) -> FutureResult: ...

@final
class MontyFileHandle:
    """A file opened inside the sandbox, surfaced as a wire value.

    Plain data holder — the sandbox never hands out a live OS file descriptor.
    Produced when a `MontyObject::FileHandle` crosses the boundary (e.g. an
    `OsCall` argument) and constructible to answer an `Open` OS call. The `mode`
    is canonicalized at construction (`'rt'` → `'r'`, `'r+b'` → `'rb+'`).
    `pydantic_monty` re-exports this same class as `MontyFileHandle`.
    """

    path: str
    mode: str
    position: int
    binary: bool
    readable: bool
    writable: bool
    def __new__(cls, path: str, mode: str, *, position: int = 0) -> MontyFileHandle: ...

# =============================================================================
# ParentRequest arms (client → sandbox)
# =============================================================================

@final
class StartSession:
    """Opens the session the child serves until `Reset`."""

    script_name: str
    limits: dict[str, int] | None
    type_check: bool
    type_check_stubs: str | None
    monty_version: str
    def __new__(
        cls,
        *,
        script_name: str = 'main.py',
        limits: Mapping[str, int] | None = None,
        type_check: bool = False,
        type_check_stubs: str | None = None,
        monty_version: str | None = None,
    ) -> StartSession: ...

@final
class Feed:
    """Executes one snippet against the session."""

    code: str
    inputs: dict[str, Value]
    mounts: list[Mount]
    skip_type_check: bool
    def __new__(
        cls,
        code: str,
        *,
        inputs: Mapping[str, Value] | None = None,
        mounts: list[Mount] | None = None,
        skip_type_check: bool = False,
    ) -> Feed: ...

@final
class ResumeCall:
    """Answers a `FunctionCall` or `OsCall` suspension."""

    call_id: int
    result: ExtFunctionResult
    def __new__(cls, call_id: int, result: ExtFunctionResult) -> ResumeCall: ...

@final
class ResumeNameLookup:
    """Answers a `NameLookup` suspension."""

    is_defined: bool
    value: Value | None
    @classmethod
    def resolved(cls, value: Value) -> ResumeNameLookup:
        """The name resolves to `value`."""

    @classmethod
    def undefined(cls) -> ResumeNameLookup:
        """The name is undefined (the child raises `NameError`)."""

@final
class ResumeFutures:
    """Answers a `ResolveFutures` suspension."""

    results: list[FutureResult]
    def __new__(cls, results: list[FutureResult]) -> ResumeFutures: ...

@final
class Dump:
    """Requests an opaque snapshot of the session state."""

    def __new__(cls) -> Dump: ...

@final
class Load:
    """Restores `Dump` state into a fresh child."""

    state: bytes
    def __new__(cls, state: bytes) -> Load: ...

@final
class Reset:
    """Ends the checkout; the child returns to no-session."""

    def __new__(cls) -> Reset: ...

@final
class Shutdown:
    """Asks the child to reply `Ok` and exit cleanly."""

    def __new__(cls) -> Shutdown: ...

# =============================================================================
# ChildEvent arms (sandbox → client)
#
# Every event also carries `total_execution_micros` / `max_duration_micros`.
# =============================================================================

@final
class Print:
    """Streamed sandbox `print()` output."""

    stream: str
    text: str
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        stream: str,
        text: str,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> Print: ...

@final
class FunctionCall:
    """Suspension: the sandbox called an external function."""

    function_name: str
    args: list[Value]
    kwargs: dict[str, Value]
    call_id: int
    method_call: bool
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        function_name: str,
        *,
        args: list[Value] | None = None,
        kwargs: Mapping[str, Value] | None = None,
        call_id: int = 0,
        method_call: bool = False,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> FunctionCall: ...

@final
class OsCall:
    """Suspension: the sandbox performed an OS operation no mount handled."""

    function_name: str
    args: list[Value]
    kwargs: dict[str, Value]
    call_id: int
    not_handled_error: RaisedException | None
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        function_name: str,
        *,
        args: list[Value] | None = None,
        kwargs: Mapping[str, Value] | None = None,
        call_id: int = 0,
        not_handled_error: RaisedException | None = None,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> OsCall: ...

@final
class NameLookup:
    """Suspension: the sandbox read an undefined name."""

    name: str
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        name: str,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> NameLookup: ...

@final
class ResolveFutures:
    """Suspension: every sandbox task is blocked on external futures."""

    pending_call_ids: list[int]
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        pending_call_ids: list[int],
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> ResolveFutures: ...

@final
class Complete:
    """Turn end: the snippet completed with this value."""

    value: Value
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        value: Value,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> Complete: ...

@final
class Error:
    """Turn end: the snippet failed with a Python exception."""

    exception: RaisedException
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        exception: RaisedException,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> Error: ...

@final
class TypingError:
    """Turn end: type checking rejected the fed snippet."""

    diagnostics: str
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        diagnostics: str,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> TypingError: ...

@final
class DumpResult:
    """Reply to `Dump`: the opaque, version-pinned snapshot bytes."""

    state: bytes
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        state: bytes,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> DumpResult: ...

@final
class Ok:
    """Generic acknowledgement for `StartSession` / `Load` / `Reset` / `Shutdown`."""

    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> Ok: ...

@final
class FatalError:
    """The child hit an unrecoverable error and exits immediately after this.
    EOF *without* a `FatalError` means the child crashed hard."""

    message: str
    total_execution_micros: int
    max_duration_micros: int | None
    def __new__(
        cls,
        message: str,
        *,
        total_execution_micros: int = 0,
        max_duration_micros: int | None = None,
    ) -> FatalError: ...

# Unions over the protocol's oneof arms (the runtime aliases live in
# `wire_protocol/__init__.py`; these mirror them for type checkers).
ParentRequest = StartSession | Feed | ResumeCall | ResumeNameLookup | ResumeFutures | Dump | Load | Reset | Shutdown
ChildEvent = (
    Print
    | FunctionCall
    | OsCall
    | NameLookup
    | ResolveFutures
    | Complete
    | Error
    | TypingError
    | DumpResult
    | Ok
    | FatalError
)
