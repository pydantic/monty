# Classes across the sandbox boundary

One concise, runnable file per behaviour, in Python and TypeScript.

## Python

Each needs a dev build of the bindings and worker (`make dev-py`), then:

```bash
uv run python examples/classes/class_instance.py
```

| File                                             | Behaviour                                                                                    |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------- |
| [`class_instance.py`](class_instance.py)         | Expose a host object with an explicit policy; identity round-trip                            |
| [`lazy_attrs.py`](lazy_attrs.py)                 | Attributes fetched from the host on demand; denial raises `AttributeError`                   |
| [`sandbox_copy.py`](sandbox_copy.py)             | Sandbox mutations stay on the sandbox copy; the host object is untouched                     |
| [`convert_value.py`](convert_value.py)           | Transform / wrap values as they cross the boundary (each wrap is retained for the session)   |
| [`class_type.py`](class_type.py)                 | Let the sandbox instantiate a host class (`init=True`)                                       |
| [`class_type_members.py`](class_type_members.py) | Class constants and classmethods without instantiation                                       |
| [`sandbox_classes.py`](sandbox_classes.py)       | Classes defined inside Monty; instances return as `MontyClassProxy`                          |
| [`sandbox_round_trip.py`](sandbox_round_trip.py) | A `MontyClassProxy` passed back resolves to the original sandbox object; freed objects raise |
| [`async_methods.py`](async_methods.py)           | `async def` methods awaited from sandbox code via `AsyncMonty`                               |

## TypeScript

The examples import `@pydantic/monty` from this repo's JS crate, so build it
(`make build-js`), install the local link, and point the workers at the
matching debug worker binary. They run directly under Node 24+ (native type
stripping and `await using`):

```bash
cd examples/classes && npm install && cd -
MONTY_BIN=$PWD/target/debug/monty node examples/classes/class_instance.ts
```

`MONTY_BIN` matters when another `monty` is on `PATH`: a worker from a
different release speaks a different protocol version and the pool reports it
as a crash. `npx tsc -p examples/classes` type-checks them.

| File                                             | Behaviour                                                                                    |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------- |
| [`class_instance.ts`](class_instance.ts)         | Expose a host object with an explicit policy; identity round-trip                            |
| [`lazy_attrs.ts`](lazy_attrs.ts)                 | Attributes fetched from the host on demand; denial raises `AttributeError`                   |
| [`class_type.ts`](class_type.ts)                 | Let the sandbox instantiate a host class (`init: true`)                                      |
| [`sandbox_classes.ts`](sandbox_classes.ts)       | Classes defined inside Monty; instances return as `MontyClassProxy`                          |
| [`sandbox_round_trip.ts`](sandbox_round_trip.ts) | A `MontyClassProxy` passed back resolves to the original sandbox object; freed objects raise |
