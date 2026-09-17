"""Host-side helpers shared by tasks: an in-memory SQLite database and a chart recorder.

Tasks build their own fixture data; these classes give them the host functions the
talks' demos exposed (`query`, `list_tables`, `describe_table`, `insert_rows`,
`table_count`, `draw_chart`) so the sandboxed code sees the same API everywhere.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

__all__ = ('ChartCall', 'ChartRecorder', 'SqliteDb')


@dataclass
class SqliteDb:
    """An in-memory SQLite database rebuilt from `seed` on every `reset()`.

    Errors come back as `[{'error': ...}]` rather than raising, as in the talks'
    `Database` class, so a stale schema in the prompt is something the code can
    recover from.
    """

    seed: Any
    """`Callable[[sqlite3.Connection], None]` that creates and fills the tables."""
    read_only: bool = False
    _conn: sqlite3.Connection = field(init=False, repr=False)

    def __post_init__(self) -> None:
        self.reset()

    def reset(self) -> None:
        """Rebuild the database, so every attempt starts from the same rows."""
        self._conn = sqlite3.connect(':memory:', check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self.seed(self._conn)
        self._conn.commit()

    def query(self, sql: str) -> list[dict[str, Any]]:
        """Run one SQL statement; rows as dicts, writes as `[{'rows_affected': n}]`."""
        if self.read_only and not sql.lstrip().lower().startswith(('select', 'with', 'pragma', 'explain')):
            return [{'error': 'OperationalError: attempt to write a readonly database'}]
        cursor = self._conn.cursor()
        try:
            cursor.execute(sql)
            if cursor.description:
                columns = [col[0] for col in cursor.description]
                return [dict(zip(columns, row, strict=False)) for row in cursor.fetchall()]
            self._conn.commit()
            return [{'rows_affected': cursor.rowcount}]
        except sqlite3.Error as exc:
            return [{'error': f'{type(exc).__name__}: {exc}'}]

    def list_tables(self) -> list[str]:
        """Names of every table."""
        rows = self._conn.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall()
        return [row[0] for row in rows]

    def describe_table(self, name: str) -> list[dict[str, str | bool]]:
        """Column name, type, nullability and primary-key flag for `name`."""
        try:
            rows = self._conn.execute(f'PRAGMA table_info({name})').fetchall()
        except sqlite3.Error as exc:
            return [{'error': f'{type(exc).__name__}: {exc}'}]
        return [{'name': r[1], 'type': r[2], 'nullable': not r[3], 'primary_key': bool(r[5])} for r in rows]

    def insert_rows(self, table: str, rows: list[dict[str, Any]]) -> dict[str, Any]:
        """Insert dicts sharing the first row's keys; `{'inserted': n}` or `{'error': ...}`."""
        if self.read_only:
            return {'error': 'OperationalError: attempt to write a readonly database'}
        if not rows:
            return {'inserted': 0}
        columns = list(rows[0])
        sql = f'INSERT INTO {table} ({", ".join(columns)}) VALUES ({", ".join("?" * len(columns))})'
        try:
            self._conn.executemany(sql, [tuple(row.get(c) for c in columns) for row in rows])
            self._conn.commit()
        except sqlite3.Error as exc:
            return {'error': f'{type(exc).__name__}: {exc}'}
        return {'inserted': len(rows)}

    def table_count(self, table: str) -> int:
        """Row count of `table`, or -1 when it does not exist."""
        try:
            return int(self._conn.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0])
        except sqlite3.Error:
            return -1


@dataclass(frozen=True)
class ChartCall:
    """One `draw_chart` call as the sandbox made it."""

    name: str
    kind: str
    x: list[Any]
    y: list[Any]
    options: dict[str, Any]


@dataclass
class ChartRecorder:
    """Records `draw_chart` calls and writes each as a small SVG, standing in for matplotlib.

    Evaluators read `calls` to check which charts were drawn with which data; the
    SVG files exist so a task can also require them on a mount.
    """

    directory: Path
    kinds: tuple[str, ...] = ('line', 'bar', 'stacked_bar', 'scatter', 'histogram', 'heatmap', 'pie')
    calls: list[ChartCall] = field(default_factory=list)

    def reset(self) -> None:
        """Forget earlier attempts' charts and delete their files."""
        self.calls.clear()
        if self.directory.is_dir():
            for file in self.directory.glob('*.svg'):
                file.unlink()

    async def draw_chart(
        self,
        x: list[Any],
        y: list[Any],
        *,
        name: str,
        kind: str = 'line',
        title: str | None = None,
        x_label: str | None = None,
        y_label: str | None = None,
        label: str | None = None,
        series: dict[str, list[float]] | None = None,
    ) -> str:
        """Draw a chart and return its file path; `kind` must be one of `kinds`."""
        if kind not in self.kinds:
            raise ValueError(f'unknown chart kind {kind!r}; use one of {", ".join(self.kinds)}')
        if kind not in {'stacked_bar', 'heatmap'} and len(x) != len(y):
            raise ValueError(f'x has {len(x)} points but y has {len(y)}')
        options = {'title': title, 'x_label': x_label, 'y_label': y_label, 'label': label, 'series': series}
        self.calls.append(ChartCall(name, kind, list(x), list(y), {k: v for k, v in options.items() if v}))
        self.directory.mkdir(parents=True, exist_ok=True)
        path = self.directory / f'{name.replace("/", "_")}.svg'
        path.write_text(_svg(kind, title or name, len(x)))
        return str(path)


def _svg(kind: str, title: str, points: int) -> str:
    """A labelled placeholder image; the data is checked from `ChartCall`, not the pixels."""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200">'
        f'<text x="10" y="20" font-size="14">{title}</text>'
        f'<text x="10" y="40" font-size="11">{kind}, {points} points</text></svg>\n'
    )
