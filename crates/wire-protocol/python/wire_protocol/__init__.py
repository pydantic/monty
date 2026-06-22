"""Encode/decode the Monty subprocess wire protocol over any transport.

The four codec functions turn the message objects below into protobuf bytes and
back, so a sandbox can be driven over a WebSocket, an HTTP request, a raw
socket, or a Docker `exec` pipe. See `websocket_plan.md` for transport recipes
and footguns.
"""

from typing import Union

from ._wire_protocol import (
    Complete,
    Dump,
    DumpResult,
    Error,
    ExtFunctionResult,
    FatalError,
    Feed,
    FunctionCall,
    FutureResult,
    Load,
    MontyFileHandle,
    Mount,
    NameLookup,
    Ok,
    OsCall,
    Print,
    RaisedException,
    Reset,
    ResolveFutures,
    ResumeCall,
    ResumeFutures,
    ResumeNameLookup,
    Shutdown,
    StackFrame,
    StartSession,
    TypingError,
    __version__,
    decode_child_event,
    decode_parent_request,
    encode_child_event,
    encode_parent_request,
)

# Unions over the protocol's oneof arms. `encode_parent_request` /
# `encode_child_event` accept any member; the decoders return one.
ParentRequest = Union[StartSession, Feed, ResumeCall, ResumeNameLookup, ResumeFutures, Dump, Load, Reset, Shutdown]
ChildEvent = Union[
    Print, FunctionCall, OsCall, NameLookup, ResolveFutures, Complete, Error, TypingError, DumpResult, Ok, FatalError
]

__all__ = [
    '__version__',
    # codec
    'encode_parent_request',
    'decode_parent_request',
    'encode_child_event',
    'decode_child_event',
    # unions
    'ParentRequest',
    'ChildEvent',
    # ParentRequest arms
    'StartSession',
    'Feed',
    'ResumeCall',
    'ResumeNameLookup',
    'ResumeFutures',
    'Dump',
    'Load',
    'Reset',
    'Shutdown',
    # ChildEvent arms
    'Print',
    'FunctionCall',
    'OsCall',
    'NameLookup',
    'ResolveFutures',
    'Complete',
    'Error',
    'TypingError',
    'DumpResult',
    'Ok',
    'FatalError',
    # payloads
    'Mount',
    'RaisedException',
    'StackFrame',
    'ExtFunctionResult',
    'FutureResult',
    'MontyFileHandle',
]
