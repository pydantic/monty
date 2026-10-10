# Host Objects

[`ClassInstance`][pydantic_monty.ClassInstance] and [`ClassType`][pydantic_monty.ClassType] put a host object, or a host class, in front of the sandbox.
Each wrapper is a policy: it lists which attributes cross eagerly, which the sandbox may fetch on demand, and which
methods it may call.
Every policy defaults to nothing, and `'all'` never exposes underscore-prefixed names.
Method calls, lazy attribute reads and construction run your code on the host, with the same authority as a
[host function](host-functions.md).

## Instances

=== "Python"

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

=== "TypeScript"

    ```ts
    import { ClassInstance, Monty } from '@pydantic/monty'

    class Person {
      constructor(
        public name: string,
        public age: number,
      ) {}
      greeting(): string {
        return `hi ${this.name}`
      }
    }

    const person = new Person('Samuel', 4)
    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const wrapper = new ClassInstance(person, { eagerAttrs: 'all', allowedMethods: ['greeting'] })
    const code = 'assert user.greeting() == "hi Samuel"\nuser'
    const result = await session.feedRun(code, { inputs: { user: wrapper } })
    console.log(result === person) // true
    ```

`eager_attrs` sends attribute values with the object, and `allowed_methods` lets the sandbox call back into the real
instance.
`allowed_methods='all'` exposes the functions the class defines, not callables stored as attributes or nested classes;
an explicit set exposes exactly the names you list.
Returning the object from sandbox code hands the host back the original object, not a copy.
Sandbox code may set attributes, on its own copy only: the host object is never touched.

## Lazy attributes

=== "Python"

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

=== "TypeScript"

    ```ts
    import { ClassInstance, Monty, MontyRuntimeError } from '@pydantic/monty'

    class Config {
      retries = 3
      api_key = 'hunter2'
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const wrapper = new ClassInstance(new Config(), { lazyAttrs: ['retries'] })
    console.log(await session.feedRun('cfg.retries', { inputs: { cfg: wrapper } })) // 3
    try {
      await session.feedRun('cfg.api_key', { inputs: { cfg: wrapper } })
    } catch (err) {
      if (!(err instanceof MontyRuntimeError)) throw err
      console.log(err.display('type-msg')) // AttributeError: 'Config' object has no attribute 'api_key'
    }
    ```

`lazy_attrs` names cross only when sandbox code reads them.
Each access suspends the sandbox and asks the host, so host-side changes stay visible.
A name outside every policy raises the usual `AttributeError` inside the sandbox.
An exception the host raises while serving the read (a property, or `convert_value`) is raised inside the sandbox,
where sandbox code can catch it; only `AttributeError` reads as absent.

## Classes

=== "Python"

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

=== "TypeScript"

    ```ts
    import { ClassType, Monty } from '@pydantic/monty'

    class Person {
      constructor(
        public name: string,
        public age: number,
      ) {}
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const wrapper = new ClassType(Person, { init: true, instanceEagerAttrs: 'all' })
    const code = 'p = Person("Ada", 36)\nassert type(p) is Person\np'
    const result = await session.feedRun(code, { inputs: { Person: wrapper } })
    console.log(result) // Person { name: 'Ada', age: 36 }
    ```

`init=True` grants construction; without it, calling the class raises `TypeError: cannot instantiate host class 'Person'` in the sandbox.
The construction runs on the host, and the new instance crosses back governed by the `instance_*` policies.
A constructed instance keeps the [`ClassType`][pydantic_monty.ClassType] that built it, so `type(p)` is the class the sandbox was given.
On a `ClassType` itself, `eager_attrs`, `lazy_attrs` and `allowed_methods` expose class constants, classmethods and
staticmethods.

## Values returned by methods

Nothing is wrapped automatically: a method that returns another object fails conversion unless a `convert_value` hook
wraps it with a policy you chose.
In Python the hook is a method on a [`ClassInstance`][pydantic_monty.ClassInstance] subclass; in JavaScript it is the `convertValue` option.

=== "Python"

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

=== "TypeScript"

    ```ts
    import { ClassInstance, Monty } from '@pydantic/monty'

    class Wallet {
      constructor(public balance: number) {}
      pay(amount: number): Wallet {
        return new Wallet(this.balance - amount)
      }
    }

    function wrapWallet(wallet: Wallet): ClassInstance {
      return new ClassInstance(wallet, {
        eagerAttrs: 'all',
        allowedMethods: ['pay'],
        convertValue: (_name, value) => (value instanceof Wallet ? wrapWallet(value) : value),
      })
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const result = await session.feedRun('w.pay(30).pay(20).balance', { inputs: { w: wrapWallet(new Wallet(100)) } })
    console.log(result) // 50
    ```

Every wrapper the hook creates is kept by the session's instance store until the session ends.
A method that returns a fresh object per call grows host memory by one entry per call, and
[`max_memory`](resource-limits.md#what-is-not-covered) does not count it.
Each call suspends, so [`max_suspensions`](resource-limits.md#suspensions) bounds instance-store growth.
Set it for untrusted code, and recycle long-lived sessions.

## Sandbox instances

=== "Python"

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

=== "TypeScript"

    ```ts
    import { Monty, MontyClassProxy } from '@pydantic/monty'

    const code = `\
    class Counter:
        def __init__(self):
            self.n = 1

    counter = Counter()
    counter
    `
    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const proxy = await session.feedRun(code)
    if (!(proxy instanceof MontyClassProxy)) throw new Error('expected a proxy')
    console.log(proxy.name, proxy.attributes) // Counter { n: 1 }
    console.log(await session.feedRun('back is counter', { inputs: { back: proxy } })) // true
    ```

A sandbox-defined instance reaches the host as a read-only [`MontyClassProxy`][pydantic_monty.MontyClassProxy] with `name`, `attributes`, `is_dataclass`
and `id`; the host cannot call its methods.
Passing the proxy back hands the sandbox its original object, and a proxy whose object the sandbox has freed raises.

## Snapshots

`feed_start` suspends on a method call or a lazy attribute read as it does on a host function: [`FunctionSnapshot`][pydantic_monty.FunctionSnapshot] and
[`NameLookupSnapshot`][pydantic_monty.NameLookupSnapshot] carry `object_id`, the uuid of the wrapper involved ([`ClassInstance.id`][pydantic_monty.ClassInstance.id] or [`ClassType.id`][pydantic_monty.ClassType.id]), not
the host object's `id()`; it is `None` for plain host functions and name lookups.
The instance store does not travel with a dump: a restored session returns a host instance as [`MontyClassProxy`][pydantic_monty.MontyClassProxy] and a
host class, `type(x)` included, as a read-only [`MontyClassTypeProxy`][pydantic_monty.MontyClassTypeProxy] (`name`, `id`, `is_dataclass`, `attributes`) that
re-enters as the same class; JavaScript has no such class and returns a plain `{ __monty_type__: 'Type', ... }` marker
instead.
On those objects, method calls and `init=True` construction raise `RuntimeError` inside the sandbox and lazy attribute
reads raise `AttributeError`.
See [what restoring carries](snapshots.md#what-restoring-does-and-does-not-carry).

Divergences from CPython objects (`type(x)`, equality, hashing, frozen dataclasses, inheritance, what `'all'` exposes,
lazy attribute errors) are listed in
[`limitations/classes.md`](limitations/classes.md).
