"""Write an SVG chart to a mounted directory — the only task with a judge.

Two things are under test. The mount: `/output` is a real host directory mounted
read-write, and the file has to actually appear there, which exercises the `os` call
path rather than just returning a string. And the drawing: bar heights must be
proportional to the data, which a predicate can check by parsing the SVG.

The rubric is deliberately narrow — legibility only, since bar heights are already
checked deterministically. A judge asked to grade correctness would add variance to
something we can measure exactly.
"""

from __future__ import annotations

import re
from pathlib import Path

from evals.harness.task import Every, Predicate, Rubric, Task
from pydantic_monty import MountDir

REVENUE = {
    'EMEA': 2830.75,
    'AMER': 3010.50,
    'APAC': 2965.75,
    'LATAM': 1120.00,
}

OUTPUT_DIR = Path(__file__).parent.parent.parent / 'reports' / 'artifacts'
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
CHART_PATH = OUTPUT_DIR / 'chart.svg'

STUBS = '''
REVENUE: dict[str, float] = {}
"""Revenue by region."""
'''

_RECT_HEIGHT = re.compile(r'<rect\b[^>]*\bheight\s*=\s*"([\d.]+)"', re.IGNORECASE)


def _reset() -> None:
    """Remove any chart left by a previous attempt, so a no-op cannot pass."""
    CHART_PATH.unlink(missing_ok=True)


def _bars_are_proportional(_result: object) -> bool:
    """Check the written SVG has one bar per region, scaled to the data.

    Reads the host-side file rather than the returned value: the task is to *write* the
    chart, and a solution that returns well-formed SVG without writing it has not done
    the job. Ratios are compared rather than absolute heights, since the chart's overall
    size is the model's choice.
    """
    if not CHART_PATH.is_file():
        return False
    heights = [float(value) for value in _RECT_HEIGHT.findall(CHART_PATH.read_text())]
    # Background or axis rects are allowed; the bars are the largest N distinct ones.
    if len(heights) < len(REVENUE):
        return False
    bars = sorted(heights, reverse=True)[: len(REVENUE)]
    values = sorted(REVENUE.values(), reverse=True)
    scale = bars[0] / values[0]
    return all(abs(bar - value * scale) <= max(1.0, 0.05 * bar) for bar, value in zip(bars, values))


REFERENCE = """
from pathlib import Path

regions = sorted(REVENUE, key=lambda name: REVENUE[name], reverse=True)
top = REVENUE[regions[0]]

width = 400
height = 240
bar_width = 60
gap = 30
chart_height = 180

parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">']
for index, region in enumerate(regions):
    value = REVENUE[region]
    bar_height = round(value / top * chart_height, 2)
    x = gap + index * (bar_width + gap)
    y = round(chart_height - bar_height + 20, 2)
    parts.append(
        f'<rect x="{x}" y="{y}" width="{bar_width}" height="{bar_height}" fill="#4767c9" />'
    )
    parts.append(
        f'<text x="{x + bar_width // 2}" y="{chart_height + 38}" font-size="12" '
        f'text-anchor="middle">{region}</text>'
    )
    parts.append(
        f'<text x="{x + bar_width // 2}" y="{y - 6}" font-size="11" '
        f'text-anchor="middle">{value:,.0f}</text>'
    )
parts.append('</svg>')
svg = '\\n'.join(parts)

Path('/output/chart.svg').write_text(svg)
svg
"""

TASK = Task(
    name='svg_bar_chart',
    category='artifacts',
    prompt=(
        'Draw a bar chart of REVENUE by region as an SVG, with the bars in descending '
        'order of revenue, each bar labelled with its region name and value. Write it to '
        '/output/chart.svg and return the SVG text.'
    ),
    stubs=STUBS,
    tools={},
    inputs={'REVENUE': REVENUE},
    mounts=[MountDir(host_path=OUTPUT_DIR, virtual_path='/output', mode='read-write')],
    expected=Every(
        (
            Predicate('bars written to /output/chart.svg and proportional to the data', _bars_are_proportional),
            Rubric(
                'The output is an SVG bar chart. Judge only presentation: every bar is '
                'labelled with its region name, values are readable, bars do not overlap '
                'each other or run outside the canvas, and the chart would be '
                'intelligible to someone who had not seen the underlying numbers. Do not '
                'check whether the bar heights are numerically correct.'
            ),
        )
    ),
    reference_solution=REFERENCE,
    traps=('Path.write_text through a mount', 'f-string format specs'),
    expected_external_calls=0,
    setup=_reset,
)
