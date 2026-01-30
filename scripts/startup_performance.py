# /// script
# requires-python = ">=3.14"
# dependencies = [
#     "daytona>=0.136.0",
#     "mcp-run-python>=0.0.22",
#     "pydantic-monty>=0.0.1",
#     "starlark-pyo3>=2025.2.5",
# ]
# ///
import asyncio
import os
import subprocess
import time
from typing import Any

from mcp_run_python import code_sandbox

from pydantic_monty import Monty

code = '1 + 1'


def run_monty():
    start = time.perf_counter()
    result = Monty('1 + 1').run()
    diff = time.perf_counter() - start
    assert result == 2, f'Unexpected result: {result!r}'
    print(f'Monty cold start time: {(diff * 1000):.3f} milliseconds')


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
    assert response.result == '2', f'Unexpected result: {response.result!r}'
    print(f'Daytona cold start time: {(diff * 1000):.3f} milliseconds')


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


if __name__ == '__main__':
    run_monty()
    run_pyodide()
    run_docker()
    run_starlark()
    run_daytona()
    run_subprocess_python()
    run_exec_python()
