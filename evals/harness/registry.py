"""Discovers task modules under `evals/tasks/`.

Tasks are modules rather than data files because their host functions are real
callables — the runner wraps them to count and time calls, which is where the
external-call and round-trip metrics come from.
"""

from __future__ import annotations

import importlib
from pathlib import Path

from .task import Task

__all__ = ('all_tasks', 'load_task', 'task_names')

TASKS_DIR = Path(__file__).parent.parent / 'tasks'
_PACKAGE = 'evals.tasks'


def task_names() -> list[str]:
    """Every `category/name` discoverable on disk, sorted."""
    names: list[str] = []
    for path in sorted(TASKS_DIR.glob('*/*.py')):
        if path.name.startswith('_'):
            continue
        names.append(f'{path.parent.name}/{path.stem}')
    return names


def load_task(qualified_name: str) -> Task:
    """Import `category/name` and return the `TASK` it exports."""
    if '/' not in qualified_name:
        raise ValueError(f'task name must be category/name, got {qualified_name!r}')
    category, name = qualified_name.split('/', 1)
    module = importlib.import_module(f'{_PACKAGE}.{category}.{name}')
    task = getattr(module, 'TASK', None)
    if not isinstance(task, Task):
        raise TypeError(f'{qualified_name} does not export a `TASK` of type Task')
    return task


def all_tasks() -> list[Task]:
    """Load every discoverable task."""
    return [load_task(name) for name in task_names()]
