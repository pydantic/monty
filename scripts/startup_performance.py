# /// script
# requires-python = ">=3.14"
# dependencies = [
#     "daytona>=0.136.0",
#     "mcp-run-python>=0.0.22",
#     "pydantic-monty>=0.0.1",
#     "starlark-pyo3>=2025.2.5",
#     "wasmtime>=38",
# ]
# ///
import asyncio
import os
import statistics
import subprocess
import time
from pathlib import Path
from typing import Any

from mcp_run_python import code_sandbox

from pydantic_monty import Monty

code = '1 + 1'

# The numbers these produce are quoted in docs/index.md, docs/alternatives.md and
# README.md; regenerate docs/img/startup-latency.svg with scripts/startup_latency_chart.py
# after re-running.


def run_monty():
    start = time.perf_counter()
    # cold start includes spawning a worker subprocess and the protocol
    # handshake — execution is always subprocess-isolated
    with Monty() as pool:
        with pool.checkout() as session:
            result = session.feed_run(code)
    diff = time.perf_counter() - start
    assert result == 2, f'Unexpected result: {result!r}'
    print(f'Monty cold start time: {(diff * 1000):.3f} milliseconds')


def run_monty_warm(rounds: int = 20):
    # the steady state of a long-running host: a worker already exists, so a
    # checkout is one message each way
    with Monty() as pool:
        with pool.checkout() as session:
            session.feed_run(code)
        samples: list[float] = []
        for _ in range(rounds):
            start = time.perf_counter()
            with pool.checkout() as session:
                result = session.feed_run(code)
            samples.append(time.perf_counter() - start)
            assert result == 2, f'Unexpected result: {result!r}'
    print(f'Monty warm pool time: {(statistics.median(samples) * 1000):.3f} milliseconds (median of {rounds})')


def run_pyodide():
    async def run() -> Any:
        async with code_sandbox(dependencies=['numpy']) as sandbox:
            return await sandbox.eval(code)

    start = time.perf_counter()
    result = asyncio.run(run())
    diff = time.perf_counter() - start
    assert result == {'status': 'success', 'output': [], 'return_value': 2}, f'Unexpected result: {result!r}'
    print(f'Pyodide cold start time: {(diff * 1000):.3f} milliseconds')


def run_docker():
    start = time.perf_counter()
    result = subprocess.run(
        ['docker', 'run', '--rm', 'python:3.14-alpine', 'python', '-c', f'print({code})'],
        capture_output=True,
        text=True,
    )
    diff = time.perf_counter() - start
    output = result.stdout.strip()
    assert output == '2', f'Unexpected result: {output!r}'
    print(f'Docker cold start time: {(diff * 1000):.3f} milliseconds')


def run_starlark():
    import starlark as sl

    start = time.perf_counter()
    glb = sl.Globals.standard()
    mod = sl.Module()
    ast = sl.parse('bench.star', code)
    result = sl.eval(mod, ast, glb)
    diff = time.perf_counter() - start
    assert result == 2, f'Unexpected result: {result!r}'
    print(f'Starlark cold start time: {(diff * 1000):.3f} milliseconds')


def run_daytona():
    from daytona import Daytona, DaytonaConfig

    api_key = os.getenv('DAYTONA_API_KEY')
    if not api_key:
        print('DAYTONA_API_KEY environment variable is not set, skipping daytona')
        return

    # Initialize the Daytona client
    daytona = Daytona(DaytonaConfig(api_key=api_key))

    start = time.perf_counter()
    response = daytona.create().process.code_run(f'print({code})')
    diff = time.perf_counter() - start
    assert response.result.strip() == '2', f'Unexpected result: {response.result!r}'
    print(f'Daytona cold start time: {(diff * 1000):.3f} milliseconds')


def run_wasmer():
    # requires wasmer to be installed, see https://docs.wasmer.io/install
    start = time.perf_counter()
    result = subprocess.run(
        ['wasmer', 'run', 'python/python', '--', '-c', f'print({code})'],
        capture_output=True,
        text=True,
    )
    diff = time.perf_counter() - start
    output = result.stdout.strip()
    assert output == '2', f'Unexpected result: {output!r}'
    print(f'Wasmer cold start time: {(diff * 1000):.3f} milliseconds')


def run_wasmtime():
    """CPython compiled to WASI, run in-process through the `wasmtime` package.

    Point `CPYTHON_WASI_DIR` at an unpacked release of
    https://github.com/brettcannon/cpython-wasi-build (`python.wasm` plus `lib/`).
    The module is compiled once to `python.cwasm` next to it, as a deployment would
    do ahead of time; the timed part is deserialising that, instantiating, and running.
    """
    from wasmtime import Engine, Linker, Module, Store, WasiConfig

    wasi_dir = os.getenv('CPYTHON_WASI_DIR')
    if not wasi_dir:
        print('CPYTHON_WASI_DIR environment variable is not set, skipping wasmtime')
        return
    wasi_path = Path(wasi_dir)
    engine = Engine()
    cwasm = wasi_path / 'python.cwasm'
    if not cwasm.exists():
        cwasm.write_bytes(Module.from_file(engine, str(wasi_path / 'python.wasm')).serialize())
    stdout = wasi_path / 'stdout.txt'

    start = time.perf_counter()
    module = Module.deserialize_file(engine, str(cwasm))
    linker = Linker(engine)
    linker.define_wasi()
    store = Store(engine)
    wasi = WasiConfig()
    wasi.argv = ('python', '-c', f'print({code})')
    wasi.preopen_dir(str(wasi_path), '/')
    wasi.env = (('PYTHONHOME', '/'),)
    wasi.stdout_file = str(stdout)
    store.set_wasi(wasi)
    instance = linker.instantiate(store, module)
    try:
        instance.exports(store)['_start'](store)
    except Exception:  # `sys.exit(0)` surfaces as a trap
        pass
    diff = time.perf_counter() - start
    output = stdout.read_text().strip()
    assert output == '2', f'Unexpected result: {output!r}'
    print(f'wasmtime (precompiled CPython) cold start time: {(diff * 1000):.3f} milliseconds')


def run_subprocess_python():
    start = time.perf_counter()
    result = subprocess.run(
        ['python', '-c', f'print({code})'],
        capture_output=True,
        text=True,
    )
    diff = time.perf_counter() - start
    output = result.stdout.strip()
    assert output == '2', f'Unexpected result: {output!r}'
    print(f'Subprocess Python cold start time: {(diff * 1000):.3f} milliseconds')


def run_exec_python():
    start = time.perf_counter()
    result = eval(code)
    diff = time.perf_counter() - start
    assert result == 2, f'Unexpected result: {result!r}'
    print(f'Exec Python cold start time: {(diff * 1000):.3f} milliseconds')


# --- indicative agent run: 10 REPL feeds ---------------------------------------
#
# What a simple code-mode agent does in one user turn: ten blocks against one
# environment, each building on the last. Monty and in-process `exec` keep the
# session; everything else has no persistent interpreter, so feed `i` re-runs
# blocks 1..i (the cheapest correct strategy those sandboxes allow).

AGENT_BLOCKS = [
    "orders = [{'sku': 'A1', 'qty': 2, 'price': 3.5}, {'sku': 'B2', 'qty': 1, 'price': 12.0}, {'sku': 'C3', 'qty': 5, 'price': 1.0}]",
    "def line_total(o):\n    return o['qty'] * o['price']",
    'totals = [line_total(o) for o in orders]',
    'grand = sum(totals)',
    "biggest = max(orders, key=line_total)['sku']",
    "import json\nsummary = json.dumps({'grand': grand, 'biggest': biggest})",
    'discount = 0.1 if grand > 10 else 0',
    'net = round(grand * (1 - discount), 2)',
    "report = f'{len(orders)} orders, total {net}, biggest {biggest}'",
    'print(report)',
]
AGENT_EXPECTED = '3 orders, total 21.6, biggest B2'


def replay(i: int) -> str:
    """Blocks 1..i as one program, for sandboxes with no persistent session."""
    return '\n'.join(AGENT_BLOCKS[:i])


def report_agent(name: str, seconds: float, output: str) -> None:
    assert output.strip() == AGENT_EXPECTED, f'{name}: unexpected output {output!r}'
    print(f'{name} agent run ({len(AGENT_BLOCKS)} REPL feeds): {seconds * 1000:.3f} milliseconds')


def agent_monty(warm: bool):
    from pydantic_monty import CollectString

    def feeds(pool: Monty) -> str:
        collector = CollectString()
        with pool.checkout() as session:
            for block in AGENT_BLOCKS:
                session.feed_run(block, print_callback=collector)
        return collector.output

    if warm:
        with Monty() as pool:
            feeds(pool)
            start = time.perf_counter()
            output = feeds(pool)
            diff = time.perf_counter() - start
        report_agent('Monty warm pool', diff, output)
    else:
        start = time.perf_counter()
        with Monty() as pool:
            output = feeds(pool)
        diff = time.perf_counter() - start
        report_agent('Monty cold start', diff, output)


def agent_wasmtime():
    from wasmtime import Engine, Linker, Module, Store, WasiConfig

    wasi_dir = os.getenv('CPYTHON_WASI_DIR')
    if not wasi_dir:
        print('CPYTHON_WASI_DIR environment variable is not set, skipping wasmtime agent run')
        return
    wasi_path = Path(wasi_dir)
    engine = Engine()
    cwasm = wasi_path / 'python.cwasm'
    if not cwasm.exists():
        cwasm.write_bytes(Module.from_file(engine, str(wasi_path / 'python.wasm')).serialize())
    stdout = wasi_path / 'stdout.txt'

    start = time.perf_counter()
    # one module, one store per feed: deserialising a new module while the previous
    # store is still alive costs ~200 ms of fresh page faults on every feed
    module = Module.deserialize_file(engine, str(cwasm))
    linker = Linker(engine)
    linker.define_wasi()
    for i in range(1, len(AGENT_BLOCKS) + 1):
        store = Store(engine)
        wasi = WasiConfig()
        wasi.argv = ('python', '-c', replay(i))
        wasi.preopen_dir(str(wasi_path), '/')
        wasi.env = (('PYTHONHOME', '/'),)
        wasi.stdout_file = str(stdout)
        store.set_wasi(wasi)
        instance = linker.instantiate(store, module)
        try:
            instance.exports(store)['_start'](store)
        except Exception:  # `sys.exit(0)` surfaces as a trap
            pass
        output = stdout.read_text()
    diff = time.perf_counter() - start
    report_agent('wasmtime (precompiled CPython, replayed)', diff, output)


def agent_docker():
    # the container is already running, as a long-lived sandbox would be; each
    # feed is one `docker exec` replaying the blocks so far
    container = subprocess.run(
        ['docker', 'run', '-d', '--rm', 'python:3.14-alpine', 'sleep', 'infinity'],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    try:
        start = time.perf_counter()
        for i in range(1, len(AGENT_BLOCKS) + 1):
            result = subprocess.run(
                ['docker', 'exec', container, 'python', '-c', replay(i)],
                capture_output=True,
                text=True,
            )
            output = result.stdout
        diff = time.perf_counter() - start
    finally:
        subprocess.run(['docker', 'stop', '-t', '0', container], capture_output=True)
    report_agent('Docker (running container, replayed)', diff, output)


def agent_pyodide():
    async def run() -> tuple[float, str]:
        async with code_sandbox() as sandbox:
            await sandbox.eval(replay(1))  # warm the sandbox, as a long-lived one would be
            start = time.perf_counter()
            for i in range(1, len(AGENT_BLOCKS) + 1):
                result = await sandbox.eval(replay(i))
            diff = time.perf_counter() - start
            assert result['status'] == 'success', result
            return diff, '\n'.join(result['output'])

    diff, output = asyncio.run(run())
    report_agent('Pyodide (replayed)', diff, output)


def agent_daytona():
    from daytona import Daytona, DaytonaConfig

    api_key = os.getenv('DAYTONA_API_KEY')
    if not api_key:
        print('DAYTONA_API_KEY environment variable is not set, skipping daytona agent run')
        return
    sandbox = Daytona(DaytonaConfig(api_key=api_key)).create()
    try:
        sandbox.process.code_run(replay(1))  # warm the sandbox
        start = time.perf_counter()
        for i in range(1, len(AGENT_BLOCKS) + 1):
            output = sandbox.process.code_run(replay(i)).result
        diff = time.perf_counter() - start
    finally:
        sandbox.delete()
    report_agent('Daytona (existing sandbox, replayed)', diff, output)


def agent_subprocess_python():
    start = time.perf_counter()
    for i in range(1, len(AGENT_BLOCKS) + 1):
        output = subprocess.run(['python', '-c', replay(i)], capture_output=True, text=True).stdout
    diff = time.perf_counter() - start
    report_agent('Subprocess Python (replayed)', diff, output)


def agent_exec_python():
    import contextlib
    import io

    namespace: dict[str, Any] = {}
    buffer = io.StringIO()
    start = time.perf_counter()
    with contextlib.redirect_stdout(buffer):
        for block in AGENT_BLOCKS:
            exec(block, namespace)
    diff = time.perf_counter() - start
    report_agent('Exec Python', diff, buffer.getvalue())


if __name__ == '__main__':
    run_monty()
    run_monty_warm()
    run_pyodide()
    run_docker()
    run_starlark()
    run_daytona()
    run_wasmer()
    run_wasmtime()
    run_subprocess_python()
    run_exec_python()

    print()
    agent_monty(warm=True)
    agent_monty(warm=False)
    agent_wasmtime()
    agent_docker()
    agent_pyodide()
    agent_daytona()
    agent_subprocess_python()
    agent_exec_python()
