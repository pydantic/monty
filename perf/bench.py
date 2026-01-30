# /// script
# requires-python = ">=3.14"
# dependencies = [
#     "daytona>=0.136.0",
#     "mcp-run-python>=0.0.22",
#     "pydantic-monty>=0.0.1",
# ]
# ///
import os
import subprocess
import time

from mcp_run_python import code_sandbox

from pydantic_monty import Monty

code = '1 + 1'


def run_monty():
    start = time.perf_counter()
    print(Monty('1 + 1').run())
    diff = time.perf_counter() - start
    print(f'Monty cold start time: {(diff * 1000):.3f} milliseconds')


async def run_pyodide():
    start = time.perf_counter()
    async with code_sandbox(dependencies=['numpy']) as sandbox:
        result = await sandbox.eval(code)
        print(result)
    diff = time.perf_counter() - start
    print(f'Pyodide cold start time: {(diff * 1000):.3f} milliseconds')


def run_docker():
    start = time.perf_counter()
    result = subprocess.run(
        ['docker', 'run', '--rm', 'python:3.14-alpine', 'python', '-c', f'print({code})'],
        capture_output=True,
        text=True,
    )
    print(result.stdout.strip())
    diff = time.perf_counter() - start
    print(f'Docker cold start time: {(diff * 1000):.3f} milliseconds')


def run_starlark():
    start = time.perf_counter()
    result = subprocess.run(
        ['./starlark-rust/target/release/starlark', '-e', f'print({code})'],
        capture_output=True,
        text=True,
    )
    print(result.stdout.strip())
    diff = time.perf_counter() - start
    print(f'Starlark cold start time: {(diff * 1000):.3f} milliseconds')


def run_daytona():
    from daytona import Daytona, DaytonaConfig

    # Initialize the Daytona client
    daytona = Daytona(DaytonaConfig(api_key=os.environ['DAYTONA_API_KEY']))

    start = time.perf_counter()
    response = daytona.create().process.code_run(f'print({code})')
    print(response.result)
    diff = time.perf_counter() - start
    print(f'Daytona cold start time: {(diff * 1000):.3f} milliseconds')


def run_subprocess_python():
    start = time.perf_counter()
    result = subprocess.run(
        ['python', '-c', f'print({code})'],
        capture_output=True,
        text=True,
    )
    print(result.stdout.strip())
    diff = time.perf_counter() - start
    print(f'Subprocess Python cold start time: {(diff * 1000):.3f} milliseconds')


if __name__ == '__main__':
    import asyncio

    run_monty()
    asyncio.run(run_pyodide())
    run_docker()
    run_starlark()
    run_daytona()
    run_subprocess_python()
