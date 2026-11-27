"""Discover and run the adapted CPython suites with native unittest."""

import io
import subprocess
import sys
import unittest
from collections.abc import Generator, Iterator
from contextlib import contextmanager
from importlib import import_module
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
from types import ModuleType


def _load_module(name: str, path: Path) -> ModuleType:
    spec = spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f'cannot load {name} from {path}')
    module = module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _test_modules() -> list[str]:
    return [name for name in list(sys.modules) if name == 'test' or name.startswith('test.')]


@contextmanager
def _load_suite(root: str, path: str, name: str) -> Generator[tuple[ModuleType, unittest.TestSuite]]:
    # Use the pinned test.support and fixtures, but keep interpreter-provided unittest and the rest of Lib.
    previous = {name: sys.modules.pop(name) for name in _test_modules()}
    try:
        _load_module('test', Path(root) / '__init__.py')
        import_module(name.rpartition('.')[0])
        module = _load_module(name, Path(path))
        loader = unittest.TestLoader()
        suite = loader.loadTestsFromModule(module)
        if loader.errors:
            raise RuntimeError('\n'.join(map(str, loader.errors)))
        if not suite.countTestCases():
            raise RuntimeError(f'{name}: unittest discovered no tests')
        yield module, suite
    finally:
        for name in _test_modules():
            del sys.modules[name]
        sys.modules.update(previous)


def _cases(suite: unittest.TestSuite) -> Iterator[unittest.TestCase]:
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from _cases(item)
        else:
            yield item


def discover(root: str, path: str, name: str) -> list[tuple[str, str, str]]:
    """Return exact IDs and their module-level class and method names for Monty."""
    with _load_suite(root, path, name) as (module, suite):
        cases: list[tuple[str, str, str]] = []
        for case in _cases(suite):
            cls = type(case)
            method = case._testMethodName
            test_id = case.id()
            if (
                not cls.__name__.isidentifier()
                or not method.isidentifier()
                or getattr(module, cls.__name__, None) is not cls
                or test_id != f'{name}.{cls.__name__}.{method}'
            ):
                raise RuntimeError(f'{test_id}: Monty requires a module-level TestCase and a named method')
            cases.append((test_id, cls.__name__, method))
        return cases


def run(root: str, path: str, name: str) -> str:
    """Run the full suite on CPython's main thread with its native environment."""
    process = subprocess.run(
        [sys.executable, '-I', '-X', 'utf8', __file__, root, path, name],
        capture_output=True,
        encoding='utf-8',
        timeout=25,
        check=False,
    )
    output = process.stdout + process.stderr
    if process.returncode:
        raise RuntimeError(output or f'{name}: CPython exited with status {process.returncode}')
    return output


def _run_suite(root: str, path: str, name: str) -> str:
    with _load_suite(root, path, name) as (_, suite):
        output = io.StringIO()
        result = unittest.TextTestRunner(stream=output, verbosity=2).run(suite)
        if not result.wasSuccessful() or not result.testsRun:
            raise RuntimeError(output.getvalue())
        return output.getvalue() if result.skipped else ''


if __name__ == '__main__':
    root, path, name = sys.argv[1:]
    print(_run_suite(root, path, name), end='')
