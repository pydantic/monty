# errno

Error numbers and available names always follow Linux's asm-generic errno definitions, regardless of the host OS.
CPython instead uses its host platform's constants, so values such as `EAGAIN` differ from macOS and Windows CPython.
Names specific to other platforms, including Windows `WSA*` names and macOS `EFTYPE`, are absent.

Aliases in `errorcode` use CPython on Linux's preferred names: `EAGAIN` for 11, `EDEADLOCK` for 35, and `ENOTSUP` for 95.
These can differ from CPython's reverse mappings on other platforms.

The [general built-in module limitations](modules.md) also apply.
Missing `errorcode` keys follow the existing [KeyError argument limitation](exceptions.md#attributes).
