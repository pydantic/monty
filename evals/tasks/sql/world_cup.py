"""Football analytics over a small World Cup database: the `pydantic/talks` workshop shape.

Two groups of tables spell team names differently, so the join has to go through
`team_meta`; `goals.minute` is text that can read `'90+3'`; `events.qualifiers` is
JSON. The answer needs all three, one `draw_chart` call, an SVG written through a
mount, and a markdown table whose whitespace is checked exactly.
"""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.evaluators import Predicate
from evals.harness.fixtures import ChartRecorder, SqliteDb
from evals.harness.task import Task
from pydantic_monty import MountDir

OUTPUT_DIR = Path(__file__).parent.parent.parent / 'reports' / 'artifacts' / 'world_cup'
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

# Results-side name, event-side name, FIFA code.
_TEAMS = [
    ('Mexico', 'Mexico', 'MEX'),
    ('South Korea', 'Republic of Korea', 'KOR'),
    ('Czech Republic', 'Czechia', 'CZE'),
    ('Bosnia & Herzegovina', 'Bosnia and Herzegovina', 'BIH'),
]
_EVENT_NAME = {results: event for results, event, _ in _TEAMS}

# (id, date, team1, team2, ft1, ft2, goals as (team, scorer, minute))
_MATCHES: list[tuple[int, str, str, str, int, int, list[tuple[str, str, str]]]] = [
    (1, '2026-06-11', 'Mexico', 'South Korea', 2, 1, [
        ('Mexico', 'Jiménez', '23'), ('South Korea', 'Son', '67'), ('Mexico', 'Lozano', '90+3'),
    ]),
    (2, '2026-06-12', 'Czech Republic', 'Bosnia & Herzegovina', 1, 1, [
        ('Czech Republic', 'Schick', '45+1'), ('Bosnia & Herzegovina', 'Džeko', '81'),
    ]),
    (3, '2026-06-17', 'Mexico', 'Czech Republic', 0, 2, [
        ('Czech Republic', 'Schick', '12'), ('Czech Republic', 'Souček', '84'),
    ]),
    (4, '2026-06-18', 'South Korea', 'Bosnia & Herzegovina', 3, 0, [
        ('South Korea', 'Son', '8'), ('South Korea', 'Hwang', '55'), ('South Korea', 'Lee', '90+5'),
    ]),
    (5, '2026-06-23', 'Bosnia & Herzegovina', 'Mexico', 1, 2, [
        ('Bosnia & Herzegovina', 'Džeko', '30'), ('Mexico', 'Jiménez', '70'), ('Mexico', 'Álvarez', '88'),
    ]),
    (6, '2026-06-24', 'South Korea', 'Czech Republic', 1, 1, [
        ('South Korea', 'Hwang', '41'), ('Czech Republic', 'Schick', '90+2'),
    ]),
]  # fmt: skip

# Shots per match as (results-side team, big chance?, goal?); every goal above is a big-chance shot too.
_EXTRA_SHOTS: dict[int, list[tuple[str, bool]]] = {
    1: [('Mexico', True), ('Mexico', False), ('South Korea', True), ('South Korea', True), ('South Korea', False)],
    2: [('Czech Republic', True), ('Czech Republic', False), ('Bosnia & Herzegovina', False)],
    3: [('Mexico', True), ('Mexico', True), ('Mexico', False), ('Czech Republic', False)],
    4: [('South Korea', False), ('Bosnia & Herzegovina', True), ('Bosnia & Herzegovina', True)],
    5: [('Bosnia & Herzegovina', True), ('Mexico', False), ('Mexico', True)],
    6: [('South Korea', True), ('South Korea', False), ('Czech Republic', True), ('Czech Republic', True)],
}


def _seed(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE teams (name TEXT PRIMARY KEY, name_normalised TEXT, group_letter TEXT, fifa_code TEXT);
        CREATE TABLE matches (id INTEGER PRIMARY KEY, stage TEXT, round TEXT, group_letter TEXT, date TEXT,
                              team1 TEXT, team2 TEXT, ft1 INTEGER, ft2 INTEGER);
        CREATE TABLE goals (match_id INTEGER, team_name TEXT, scorer TEXT, minute TEXT, penalty INTEGER, own_goal INTEGER);
        CREATE TABLE team_meta (whoscored_name TEXT PRIMARY KEY, display_name TEXT, group_letter TEXT);
        CREATE TABLE event_matches (match_id INTEGER PRIMARY KEY, home_team TEXT, away_team TEXT, wc_match_id INTEGER);
        CREATE TABLE events (match_id INTEGER, event_id INTEGER, team TEXT, player TEXT, event TEXT,
                             is_shot INTEGER, is_goal INTEGER, x REAL, y REAL, qualifiers TEXT);
        """
    )
    for results, event, code in _TEAMS:
        conn.execute('INSERT INTO teams VALUES (?, ?, ?, ?)', (results, None, 'A', code))
        conn.execute('INSERT INTO team_meta VALUES (?, ?, ?)', (event, results, 'A'))
    event_id = 1000
    for match_id, date, team1, team2, ft1, ft2, goals in _MATCHES:
        conn.execute(
            'INSERT INTO matches VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)',
            (match_id, 'group', f'Matchday {match_id}', 'A', date, team1, team2, ft1, ft2),
        )
        conn.execute(
            'INSERT INTO event_matches VALUES (?, ?, ?, ?)',
            (500 + match_id, _EVENT_NAME[team1], _EVENT_NAME[team2], match_id),
        )
        for team, scorer, minute in goals:
            conn.execute('INSERT INTO goals VALUES (?, ?, ?, ?, 0, 0)', (match_id, team, scorer, minute))
            event_id += 1
            conn.execute(
                'INSERT INTO events VALUES (?, ?, ?, ?, ?, 1, 1, ?, ?, ?)',
                (500 + match_id, event_id, _EVENT_NAME[team], scorer, 'Goal', 88.0, 50.0, '{"BigChance": true}'),
            )
        for team, big in _EXTRA_SHOTS[match_id]:
            event_id += 1
            qualifiers = '{"BigChance": true, "RightFoot": true}' if big else '{"RightFoot": true}'
            conn.execute(
                'INSERT INTO events VALUES (?, ?, ?, ?, ?, 1, 0, ?, ?, ?)',
                (500 + match_id, event_id, _EVENT_NAME[team], 'Player', 'MissedShots', 75.0, 45.0, qualifiers),
            )
        for i in range(3):
            event_id += 1
            conn.execute(
                'INSERT INTO events VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?)',
                (500 + match_id, event_id, _EVENT_NAME[team1], 'Player', 'Pass', 50.0 + i, 50.0, None),
            )


DB = SqliteDb(_seed, read_only=True)
CHARTS = ChartRecorder(OUTPUT_DIR / 'charts')
SVG_PATH = OUTPUT_DIR / 'conversion.svg'


def _minute(text: str) -> int:
    base, _, extra = text.partition('+')
    return int(base) + (int(extra) if extra else 0)


def _expected_rows() -> list[dict[str, Any]]:
    """Per team: big chances (events, group stage), goals and late goals (goals table)."""
    rows: list[dict[str, Any]] = []
    for results, _, _ in _TEAMS:
        big = sum(1 for m in _MATCHES for team, _, _ in m[6] if team == results)
        big += sum(1 for shots in _EXTRA_SHOTS.values() for team, is_big in shots if team == results and is_big)
        goals = [minute for m in _MATCHES for team, _, minute in m[6] if team == results]
        rows.append(
            {
                'team': results,
                'big_chances': big,
                'goals': len(goals),
                'late': sum(1 for minute in goals if _minute(minute) >= 80),
                'conversion': len(goals) / big,
            }
        )
    return sorted(rows, key=lambda r: (-r['conversion'], r['team']))


def _table(rows: list[dict[str, Any]]) -> str:
    cells = [['Team', 'Big chances', 'Goals', 'Late', 'Conversion']]
    for r in rows:
        cells.append(
            [r['team'], str(r['big_chances']), str(r['goals']), str(r['late']), f'{r["conversion"] * 100:.1f}%']
        )
    widths = [max(len(row[i]) for row in cells) for i in range(5)]

    def line(row: list[str]) -> str:
        return f'| {row[0]:<{widths[0]}} | ' + ' | '.join(f'{row[i]:>{widths[i]}}' for i in range(1, 5)) + ' |'

    sep = f'| :{"-" * (widths[0] - 1)} | ' + ' | '.join(f'{"-" * (widths[i] - 1)}:' for i in range(1, 5)) + ' |'
    return '\n'.join([line(cells[0]), sep, *(line(row) for row in cells[1:])])


_ROWS = _expected_rows()
EXPECTED = {'best': _ROWS[0]['team'], 'table': _table(_ROWS)}

_RECT_HEIGHT = re.compile(r'<rect\b[^>]*\bheight\s*=\s*"([\d.]+)"')


def _reset() -> None:
    DB.reset()
    CHARTS.reset()
    SVG_PATH.unlink(missing_ok=True)


def _chart_and_svg(_result: object) -> bool:
    """One bar chart of big chances per team, and an SVG with a bar per team scaled to conversion."""
    calls = [c for c in CHARTS.calls if c.name == 'big_chances']
    if len(calls) != 1 or calls[0].kind != 'bar':
        return False
    drawn = dict(zip(calls[0].x, calls[0].y, strict=False))
    if any(drawn.get(r['team']) != r['big_chances'] for r in _ROWS):
        return False
    if not SVG_PATH.is_file():
        return False
    heights = sorted((float(h) for h in _RECT_HEIGHT.findall(SVG_PATH.read_text())), reverse=True)[: len(_ROWS)]
    if len(heights) < len(_ROWS):
        return False
    scale = heights[0] / _ROWS[0]['conversion']
    return all(abs(h - r['conversion'] * scale) <= max(1.0, 0.05 * h) for h, r in zip(heights, _ROWS, strict=False))


STUBS = '''
from typing import Any

def query(sql: str) -> list[dict[str, Any]]:
    """Run a read-only SQL query; rows as dicts, or `[{"error": ...}]` when it fails."""
    ...

def list_tables() -> list[str]: ...

def describe_table(name: str) -> list[dict[str, Any]]:
    """Column name, type, nullable and primary_key for each column of `name`."""
    ...

async def draw_chart(
    x: list[Any],
    y: list[Any],
    *,
    name: str,
    kind: str = 'line',
    title: str | None = None,
    x_label: str | None = None,
    y_label: str | None = None,
    label: str | None = None,
    series: dict[str, list[Any]] | None = None,
) -> str:
    """Draw a chart (`kind` is line, bar, stacked_bar, scatter, histogram, heatmap or pie) and return its file path."""
    ...
'''

PROMPT = """
You have read-only SQL access to a 2026 World Cup database. Tables:
`teams(name, name_normalised, group_letter, fifa_code)`, `matches(id, stage, round, group_letter, date, team1, team2, ft1, ft2)`,
`goals(match_id, team_name, scorer, minute, penalty, own_goal)`, `team_meta(whoscored_name, display_name, group_letter)`,
`event_matches(match_id, home_team, away_team, wc_match_id)` and
`events(match_id, event_id, team, player, event, is_shot, is_goal, x, y, qualifiers)`.
`matches`, `goals` and `teams` use one spelling of team names; `event_matches` and `events` use another, and
`team_meta.display_name` maps the event-side `whoscored_name` to the results-side name. Never join the two groups on
team name directly. `goals.minute` is text and can carry stoppage time like `'90+3'` (that is minute 93).
`events.qualifiers` is a JSON object; a shot is a big chance when it has the key `BigChance`.

For each Group A team compute: big chances (shots, `is_shot = 1`, with `BigChance`), goals (from `goals`), late goals
(minute 80 or later, stoppage time included) and conversion = goals / big chances.
Draw a bar chart of big chances per team with `draw_chart(kind='bar', name='big_chances')`, x being the results-side
team names. Write `/output/conversion.svg` containing one `<rect>` per team whose height is proportional to its
conversion. Return a dict with "best" (the team with the highest conversion) and "table": a markdown table with
columns Team, Big chances, Goals, Late, Conversion (percentage with one decimal, e.g. `37.5%`), rows sorted by
conversion descending then team name, Team left-aligned and the rest right-aligned, every cell padded to the widest
cell in its column including the header, and a separator row of the form `| :--- | ---: | ---: | ---: | ---: |` with
each dash run matching its column width.
"""

REFERENCE = """
import json
from pathlib import Path

meta = {row['whoscored_name']: row['display_name'] for row in query('SELECT whoscored_name, display_name FROM team_meta')}
teams = [row['name'] for row in query("SELECT name FROM teams WHERE group_letter = 'A' ORDER BY name")]

shots = query('SELECT team, qualifiers FROM events WHERE is_shot = 1')
big = {}
for shot in shots:
    q = json.loads(shot['qualifiers']) if shot['qualifiers'] else {}
    if 'BigChance' in q:
        name = meta[shot['team']]
        big[name] = big.get(name, 0) + 1

def minute_of(text):
    parts = text.split('+')
    return int(parts[0]) + (int(parts[1]) if len(parts) > 1 else 0)

goals = {}
late = {}
for row in query('SELECT team_name, minute FROM goals'):
    goals[row['team_name']] = goals.get(row['team_name'], 0) + 1
    if minute_of(row['minute']) >= 80:
        late[row['team_name']] = late.get(row['team_name'], 0) + 1

rows = []
for team in teams:
    b = big.get(team, 0)
    g = goals.get(team, 0)
    rows.append({'team': team, 'big': b, 'goals': g, 'late': late.get(team, 0), 'conv': g / b if b else 0.0})
rows = sorted(rows, key=lambda r: (-r['conv'], r['team']))

await draw_chart([r['team'] for r in rows], [r['big'] for r in rows], name='big_chances', kind='bar', title='Big chances')

top = rows[0]['conv']
parts = ['<svg xmlns="http://www.w3.org/2000/svg" width="400" height="220">']
for i, r in enumerate(rows):
    h = round(r['conv'] / top * 180, 2)
    parts.append(f'<rect x="{20 + i * 90}" y="{round(200 - h, 2)}" width="60" height="{h}" fill="#4767c9" />')
parts.append('</svg>')
Path('/output/conversion.svg').write_text('\\n'.join(parts))

cells = [['Team', 'Big chances', 'Goals', 'Late', 'Conversion']]
for r in rows:
    cells.append([str(r['team']), str(r['big']), str(r['goals']), str(r['late']), f'{r["conv"] * 100:.1f}%'])
widths = []
for col in range(5):
    widest = 0
    for row in cells:
        if len(row[col]) > widest:
            widest = len(row[col])
    widths.append(widest)

def fmt(row):
    out = f'| {row[0]:<{widths[0]}} |'
    for col in range(1, 5):
        out = out + f' {row[col]:>{widths[col]}} |'
    return out

sep = f'| :{"-" * (widths[0] - 1)} |'
for col in range(1, 5):
    sep = sep + f' {"-" * (widths[col] - 1)}: |'
lines = [fmt(cells[0]), sep]
for row in cells[1:]:
    lines.append(fmt(row))

{'best': rows[0]['team'], 'table': '\\n'.join(lines)}
"""

TASK = Task(
    name='world_cup',
    category='sql',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={
        'query': DB.query,
        'list_tables': DB.list_tables,
        'describe_table': DB.describe_table,
        'draw_chart': CHARTS.draw_chart,
    },
    mounts=[MountDir(host_path=OUTPUT_DIR, virtual_path='/output', mode='read-write')],
    expected=EXPECTED,
    evaluators=(
        EqualsExpected(),
        Predicate('bar chart of big chances drawn and conversion SVG written', _chart_and_svg),
    ),
    reference_solution=REFERENCE,
    traps=('joining event and results tables on team name', 'int() on "90+3"', 'str.format for the table'),
    setup=_reset,
)
