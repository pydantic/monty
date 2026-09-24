"""Render docs/img/startup-latency.svg from the numbers in `ROWS`.

The numbers come from `scripts/startup_performance.py`; update them here after re-running it,
then `uv run scripts/startup_latency_chart.py`. The same figures are quoted in `docs/index.md`,
`docs/alternatives.md` and `README.md`, so change all four together.

The axis is linear, so the Monty bar is a sliver: that is the point of the chart.
Other bars are split into the new sandbox (blue) and the agent run (amber); the table under the chart
explains the halves, so the SVG has no legend.
The SVG uses mid-grey text and axes only, so it reads on both light and dark backgrounds.
"""

from __future__ import annotations

from pathlib import Path

# (label, new sandbox ms, agent run ms, is_monty): the "New sandbox" and "Agent run" columns
# of the table in docs/index.md; each bar is their sum, the "Combined" column
ROWS: list[tuple[str, float, float, bool]] = [
    ('OSS Monty', 0.8, 0.4, True),
    ('Full Monty', 1.6, 3.9, True),
    ('WASI / wasmtime', 16, 180, False),
    ('Docker', 195, 700, False),
    ('Sandboxing service', 1500, 400, False),
    ('Pyodide', 2700, 35, False),
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
CAPTION = 'Combined new sandbox + agent run'

TEXT = '#8a8f98'
MONTY_BAR = '#e520e9'
NEW_SANDBOX_BAR = '#4a7fd4'
AGENT_RUN_BAR = '#e39b3b'
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
    for i, (label, new_ms, run_ms, is_monty) in enumerate(ROWS):
        y = MARGIN_TOP + i * (BAR_HEIGHT + ROW_GAP)
        total_ms = new_ms + run_ms
        bar_w = max(2.0, x_for(total_ms) - LABEL_WIDTH)
        parts.append(
            f"<text x='{LABEL_WIDTH - 10}' y='{y + BAR_HEIGHT * 0.7:.1f}' text-anchor='end' "
            f"fill='{TEXT}' font-size='14' {FONT}>{label}</text>"
        )
        if is_monty:
            parts.append(bar(y, bar_w, MONTY_BAR))
        else:
            parts.extend(split_bar(y, bar_w, x_for(new_ms) - LABEL_WIDTH))
        parts.append(
            f"<text x='{LABEL_WIDTH + bar_w + 8:.1f}' y='{y + BAR_HEIGHT * 0.7:.1f}' "
            f"fill='{TEXT}' font-size='14' {FONT}>{fmt_ms(round_sig(total_ms))}</text>"
        )
    parts.append('</svg>')
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text('\n'.join(parts) + '\n')
    print(f'wrote {OUTPUT}')


def split_bar(y: float, bar_w: float, new_w: float) -> list[str]:
    """A bar in the agent-run colour with the new-sandbox segment, a plain rect, drawn over its left end."""
    return [
        bar(y, bar_w, AGENT_RUN_BAR),
        f"<rect x='{LABEL_WIDTH}' y='{y}' width='{new_w:.1f}' height='{BAR_HEIGHT}' fill='{NEW_SANDBOX_BAR}'/>",
    ]


def bar(y: float, width: float, colour: str) -> str:
    """A bar starting at the axis: square on the left where it meets the axis, rounded on the right."""
    r = min(3.0, width / 2)
    x0, x1, y1 = LABEL_WIDTH, LABEL_WIDTH + width, y + BAR_HEIGHT
    return (
        f"<path d='M{x0},{y} H{x1 - r:.1f} A{r:.1f},{r:.1f} 0 0 1 {x1:.1f},{y + r:.1f} V{y1 - r:.1f} "
        f"A{r:.1f},{r:.1f} 0 0 1 {x1 - r:.1f},{y1} H{x0} Z' fill='{colour}'/>"
    )


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


def round_sig(ms: float) -> float:
    """Round to two significant figures, as the Combined column is: 895 -> 900, 2735 -> 2700."""
    return float(f'{ms:.2g}')


def fmt_ms(ms: float) -> str:
    """`0.08 ms`, `4.9 ms`, `16 ms`, `2,800 ms`: a decimal only below 10 ms, thousands separated."""
    if ms < 10:
        return f'{ms:g} ms'
    return f'{ms:,.0f} ms'


if __name__ == '__main__':
    main()
