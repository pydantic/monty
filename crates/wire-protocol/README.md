# Python Wire Protocol

Encode and decode the [Monty](https://github.com/pydantic/monty) subprocess
wire protocol from Python, so a sandbox can be driven over **any** transport —
a WebSocket, an HTTP request, a raw TCP/Unix socket, or a Docker `exec` pipe —
not just a local subprocess pipe.

The package exposes four codec functions:

```python
from wire_protocol import (
    encode_parent_request,  # parent → child: build bytes to send to the sandbox
    decode_parent_request,  # server side: decode a request from a client
    encode_child_event,     # child → parent: build bytes to send back to the client
    decode_child_event,     # client side: decode an event/response from the sandbox
)
```
