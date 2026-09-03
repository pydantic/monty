"""Sandbox instances re-enter the sandbox by identity.

A `MontyClassProxy` carries the instance's `id`. Passing it back — as an input
or an external-function result — hands the sandbox its ORIGINAL object, not a
copy built from `attributes`; a proxy whose object the sandbox has freed raises.
"""

from pydantic_monty import Monty, MontyClassProxy, MontyRuntimeError

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run('class Counter:\n    def __init__(self):\n        self.n = 1\ncounter = Counter()')
        proxy = session.feed_run('counter')
        assert isinstance(proxy, MontyClassProxy)

        # Back in as an input: same object, host-side edits to `attributes` are ignored.
        proxy.attributes['n'] = 99
        assert session.feed_run('back is counter and back.n == 1', inputs={'back': proxy}) is True

        # Back in as an external-function result.
        def echo(value: object) -> object:
            return value

        assert session.feed_run('echo(counter) is counter', external_lookup={'echo': echo}) is True

        # Once the sandbox drops its last reference (inputs persist as session
        # globals, so `back` must go too), the proxy no longer resolves.
        session.feed_run('counter = back = None')
        try:
            session.feed_run('back', inputs={'back': proxy})
        except MontyRuntimeError as exc:
            print(f'freed object rejected: {exc}')
        else:
            raise AssertionError('expected the freed proxy to be rejected')

print(f'proxy id {proxy.id} resolved to the original sandbox object')
