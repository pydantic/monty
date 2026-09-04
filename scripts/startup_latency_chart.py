"""Render docs/img/startup-latency.svg from the numbers in `ROWS`.

The numbers come from `scripts/startup_performance.py`; update them here after re-running it,
then `uv run scripts/startup_latency_chart.py`. The same figures are quoted in `docs/index.md`,
`docs/alternatives.md` and `README.md`, so change all four together.

The axis is linear, so the Monty bar is a sliver: that is the point of the chart.
The SVG uses mid-grey text and axes only, so it reads on both light and dark backgrounds.
"""

from __future__ import annotations

from pathlib import Path

# (label, milliseconds, is_monty): cold start plus a warm agent run of 10 REPL
# commands, the "Combined" column of the table in docs/index.md
ROWS: list[tuple[str, float, bool]] = [
    ('Monty', 5, True),
    ('Full Monty', 7, True),
    ('WASI / wasmtime', 200, False),
    ('Docker', 900, False),
    ('Sandboxing service', 1900, False),
    ('Pyodide', 2700, False),
]

OUTPUT = Path(__file__).parent.parent / 'docs' / 'img' / 'startup-latency.svg'

WIDTH = 760
LABEL_WIDTH = 150
BAR_HEIGHT = 22
ROW_GAP = 12
MARGIN_TOP = 44
MARGIN_BOTTOM = 40
AXIS_MAX_MS = 3000
AXIS_STEP_MS = 500
PLOT_WIDTH = WIDTH - LABEL_WIDTH - 90
CAPTION = 'Combined cold start + agent run'

TEXT = '#8a8f98'
MONTY_BAR = '#e520e9'
OTHER_BAR = '#7c8592'
FONT = "font-family='ui-sans-serif, system-ui, sans-serif'"


def main() -> None:
    """Write the SVG; a single `print` reports where it went."""
    height = MARGIN_TOP + len(ROWS) * (BAR_HEIGHT + ROW_GAP) + MARGIN_BOTTOM
    parts = [
        f"<svg xmlns='http://www.w3.org/2000/svg' width='{WIDTH}' height='{height}' "
        f"viewBox='0 0 {WIDTH} {height}' role='img' aria-labelledby='title'>",
        "<title id='title'>Time to create a sandbox and run 10 REPL commands in it</title>",
        f"<text x='{LABEL_WIDTH + PLOT_WIDTH / 2:.1f}' y='18' text-anchor='middle' fill='{TEXT}' "
        f"font-size='15' font-weight='600' {FONT}>{CAPTION}</text>",
    ]
    parts.extend(axis(height))
    for i, (label, ms, is_monty) in enumerate(ROWS):
        y = MARGIN_TOP + i * (BAR_HEIGHT + ROW_GAP)
        bar_w = max(2.0, x_for(ms) - LABEL_WIDTH)
        colour = MONTY_BAR if is_monty else OTHER_BAR
        parts.append(
            f"<text x='{LABEL_WIDTH - 10}' y='{y + BAR_HEIGHT * 0.7:.1f}' text-anchor='end' "
            f"fill='{TEXT}' font-size='14' {FONT}>{label}</text>"
        )
        parts.append(
            f"<rect x='{LABEL_WIDTH}' y='{y}' width='{bar_w:.1f}' height='{BAR_HEIGHT}' rx='3' fill='{colour}'/>"
        )
        parts.append(
            f"<text x='{LABEL_WIDTH + bar_w + 8:.1f}' y='{y + BAR_HEIGHT * 0.7:.1f}' "
            f"fill='{TEXT}' font-size='14' {FONT}>{fmt_ms(ms)}</text>"
        )
    parts.append('</svg>')
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text('\n'.join(parts) + '\n')
    print(f'wrote {OUTPUT}')


def axis(height: int) -> list[str]:
    """Gridlines every `AXIS_STEP_MS`, labelled along the bottom."""
    parts: list[str] = []
    bottom = height - MARGIN_BOTTOM + 6
    for tick in range(0, AXIS_MAX_MS + 1, AXIS_STEP_MS):
        x = x_for(tick)
        parts.append(
            f"<line x1='{x:.1f}' y1='{MARGIN_TOP - 8}' x2='{x:.1f}' y2='{bottom}' stroke='{TEXT}' stroke-opacity='0.3'/>"
        )
        parts.append(
            f"<text x='{x:.1f}' y='{bottom + 18}' text-anchor='middle' fill='{TEXT}' font-size='12' {FONT}>{fmt_ms(tick)}</text>"
        )
    return parts


def x_for(ms: float) -> float:
    """Map milliseconds onto the plot's linear x axis."""
    return LABEL_WIDTH + PLOT_WIDTH * ms / AXIS_MAX_MS


def fmt_ms(ms: float) -> str:
    """`0.08 ms`, `7 ms`, `2,800 ms`: no trailing zeros, thousands separated."""
    if ms < 1:
        return f'{ms:g} ms'
    return f'{ms:,.0f} ms'


if __name__ == '__main__':
    main()
