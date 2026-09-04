# Host Objects

`ClassInstance` and `ClassType` put a host object, or a host class, in front of the sandbox.
Each wrapper is a policy: it lists which attributes cross eagerly, which the sandbox may fetch on demand, and which
methods it may call.
Every policy defaults to nothing, and `'all'` never exposes underscore-prefixed names.
Method calls, lazy attribute reads and construction run your code on the host, with the same authority as a
[host function](host-functions.md).

## Instances

```python
from dataclasses import dataclass

from pydantic_monty import ClassInstance, Monty


@dataclass
class Person:
    name: str
    age: int

    def greeting(self) -> str:
        return f'hi {self.name}'


person = Person(name='Samuel', age=4)
with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassInstance(person, eager_attrs='all', allowed_methods={'greeting'})
        code = 'assert user.greeting() == "hi Samuel"\nuser'
        result = session.feed_run(code, inputs={'user': wrapper})
print(result is person)
#> True
```

`eager_attrs` sends attribute values with the object, and `allowed_methods` lets the sandbox call back into the real
instance.
`allowed_methods='all'` exposes the functions the class defines, not callables stored as attributes or nested classes;
an explicit set exposes exactly the names you list.
Returning the object from sandbox code hands the host back the original object, not a copy.
Sandbox code may set attributes, on its own copy only: the host object is never touched.

## Lazy attributes

```python
from pydantic_monty import ClassInstance, Monty, MontyRuntimeError


class Config:
    def __init__(self) -> None:
        self.retries = 3
        self.api_key = 'hunter2'


with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassInstance(Config(), lazy_attrs={'retries'})
        print(session.feed_run('cfg.retries', inputs={'cfg': wrapper}))
        #> 3
        try:
            session.feed_run('cfg.api_key', inputs={'cfg': wrapper})
        except MontyRuntimeError as exc:
            print(exc)
            #> AttributeError: 'Config' object has no attribute 'api_key'
```

`lazy_attrs` names cross only when sandbox code reads them.
Each access suspends the sandbox and asks the host, so host-side changes stay visible.
A name outside every policy raises the usual `AttributeError` inside the sandbox.
An exception the host raises while serving the read (a property, or `convert_value`) is raised inside the sandbox,
where sandbox code can catch it; only `AttributeError` reads as absent.

## Classes

```python
from dataclasses import dataclass

from pydantic_monty import ClassType, Monty


@dataclass
class Person:
    name: str
    age: int


with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassType(Person, init=True, instance_eager_attrs='all')
        code = 'p = Person("Ada", 36)\nassert type(p) is Person\np'
        result = session.feed_run(code, inputs={'Person': wrapper})
print(result)
#> Person(name='Ada', age=36)
```

`init=True` grants construction; without it, calling the class raises `TypeError: cannot instantiate host class 'Person'` in the sandbox.
The construction runs on the host, and the new instance crosses back governed by the `instance_*` policies.
A constructed instance keeps the `ClassType` that built it, so `type(p)` is the class the sandbox was given.
On a `ClassType` itself, `eager_attrs`, `lazy_attrs` and `allowed_methods` expose class constants, classmethods and
staticmethods.

## Values returned by methods

Nothing is wrapped automatically: a method that returns another object fails conversion unless a `convert_value` hook
wraps it with a policy you chose.

```python
from dataclasses import dataclass
from typing import Any

from pydantic_monty import ClassInstance, Monty


@dataclass
class Wallet:
    balance: int

    def pay(self, amount: int) -> 'Wallet':
        return Wallet(balance=self.balance - amount)


class WalletWrapper(ClassInstance):
    def convert_value(self, /, name: str, value: Any) -> Any:
        if isinstance(value, Wallet):
            return WalletWrapper(value, eager_attrs='all', allowed_methods={'pay'})
        return value


with Monty() as pool:
    with pool.checkout() as session:
        wallet = WalletWrapper(Wallet(100), eager_attrs='all', allowed_methods={'pay'})
        result = session.feed_run('w.pay(30).pay(20).balance', inputs={'w': wallet})
print(result)
#> 50
```

Every wrapper the hook creates is kept by the session's instance store until the session ends.
A method that returns a fresh object per call grows host memory by one entry per call, and
[`max_memory`](resource-limits.md#what-is-not-covered) does not count it.
Each call suspends, so [`max_suspensions`](resource-limits.md#suspensions) bounds instance-store growth.
Set it for untrusted code, and recycle long-lived sessions.

## Sandbox instances

```python
from pydantic_monty import Monty, MontyClassProxy

CODE = """\
class Counter:
    def __init__(self):
        self.n = 1

counter = Counter()
counter
"""
with Monty() as pool:
    with pool.checkout() as session:
        proxy = session.feed_run(CODE)
        print(isinstance(proxy, MontyClassProxy), proxy.name, proxy.attributes)
        #> True Counter {'n': 1}
        print(session.feed_run('back is counter', inputs={'back': proxy}))
        #> True
```

A sandbox-defined instance reaches the host as a read-only `MontyClassProxy` with `name`, `attributes`, `is_dataclass`
and `id`; the host cannot call its methods.
Passing the proxy back hands the sandbox its original object, and a proxy whose object the sandbox has freed raises.

## Snapshots

`feed_start` suspends on a method call or a lazy attribute read as it does on a host function: `FunctionSnapshot` and
`NameLookupSnapshot` carry `object_id`, the uuid of the wrapper involved (`ClassInstance.id` or `ClassType.id`), not
the host object's `id()`; it is `None` for plain host functions and name lookups.
The instance store does not travel with a dump: a restored session returns a host instance as `MontyClassProxy` and a
host class, `type(x)` included, as a read-only `MontyClassTypeProxy` (`name`, `id`, `is_dataclass`, `attributes`) that
re-enters as the same class.
On those objects, method calls and `init=True` construction raise `RuntimeError` inside the sandbox and lazy attribute
reads raise `AttributeError`.
See [what restoring carries](snapshots.md#what-restoring-does-and-does-not-carry).

## JavaScript

The JavaScript package mirrors this API: `ClassInstance` and `ClassType` take `eagerAttrs`, `lazyAttrs`,
`allowedMethods`, `init`, the `instance*` policies, `convertValue` and `name`.
See the [JavaScript quickstart](quickstart/javascript.md#host-objects).

Divergences from CPython objects (`type(x)`, equality, hashing, frozen dataclasses, inheritance, what `'all'` exposes,
lazy attribute errors) are listed in
[`limitations/classes.md`](limitations/classes.md).
