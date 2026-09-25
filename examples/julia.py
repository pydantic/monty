"""Draw a Julia set as ASCII art.

Each character cell maps to a point z0 in the complex plane; the point is
iterated through z = z**2 + c and shaded by how quickly it escapes.
"""

import sys

# Dark to bright: points that escape quickly are drawn light, points that
# stay bounded for every iteration get the densest character.
SHADES = ' .:-=+*#%@'
MAX_ITER = 100


def escape_count(z: complex, c: complex) -> int:
    """Number of iterations before `|z| > 2`, capped at `MAX_ITER`."""
    for i in range(MAX_ITER):
        if abs(z) > 2:
            return i
        z = z * z + c
    return MAX_ITER


def render(c: complex, width: int, height: int) -> str:
    """Render the Julia set for `c` over the square `[-1.5, 1.5]` as text."""
    rows: list[str] = []
    for row in range(height):
        # Terminal cells are about twice as tall as they are wide, so the
        # imaginary axis uses half as many cells per unit as the real axis.
        im = 1.5 - 3.0 * row / (height - 1)
        line: list[str] = []
        for col in range(width):
            re = -1.5 + 3.0 * col / (width - 1)
            n = escape_count(complex(re, im), c)
            shade = SHADES[-1] if n == MAX_ITER else SHADES[n * (len(SHADES) - 1) // MAX_ITER]
            line.append(shade)
        rows.append(''.join(line))
    return '\n'.join(rows)


def main(argv: list[str]) -> None:
    c = complex(-0.7, 0.27015)
    width, height = 80, 40
    if len(argv) >= 3:
        c = complex(float(argv[1]), float(argv[2]))
    if len(argv) >= 5:
        width, height = int(argv[3]), int(argv[4])
    print(render(c, width, height))


if __name__ == '__main__':
    main(sys.argv)
