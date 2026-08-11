"""Example usage of the Monty Python bindings."""

import pydantic_monty

with pydantic_monty.Monty() as pool:
    # Basic execution - simple expression
    with pool.checkout() as session:
        print(f'Basic: {session.feed_run("1 + 2 * 3")!r}')  # 7

    # Using input variables, with session state persisting between feeds
    with pool.checkout() as session:
        print(f'Inputs: {session.feed_run("x + y", inputs={"x": 10, "y": 20})}')  # 30
        print(f'Reuse: {session.feed_run("x + y", inputs={"x": 100, "y": 200})}')  # 300

    # With resource limits (enforced inside the worker)
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0, max_memory=1024 * 1024)
    with pool.checkout(limits=limits) as session:
        print(f'With limits: {session.feed_run("x * y * z", inputs={"x": 2, "y": 3, "z": 4})}')  # 24

    # External function callbacks
    def fetch(url: str) -> str:
        return f'Fetched: {url}'

    with pool.checkout() as session:
        result = session.feed_run('fetch("https://example.com")', external_lookup={'fetch': fetch})
        print(f'External: {result}')

    # Print output is forwarded to Python stdout
    with pool.checkout() as session:
        session.feed_run('print("Hello from Monty!")')

    # Exception handling
    with pool.checkout() as session:
        try:
            session.feed_run('1 / 0')
        except pydantic_monty.MontyRuntimeError as e:
            print(f'Caught: {e.display(format="type-msg")}')
