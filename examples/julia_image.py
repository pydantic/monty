"""Render a Julia set in the sandbox and write it out as a PNG from the host.

The sandbox computes one escape count per pixel and returns them as a flat
`list[int]`; the host maps those counts through a colour palette with Pillow
and saves the image. Colouring and encoding stay on the host because the
sandbox cannot build `bytes` from ints and a flat list of ints crosses the
boundary several times faster than a list of `(r, g, b)` tuples.

Run with `uv run --group examples python examples/julia_image.py --help` for the options.
"""

import argparse
import time
from pathlib import Path

from PIL import Image

import pydantic_monty

# Points that never escape get index MAX_ITER, which the palette paints black.
MAX_ITER = 255

SANDBOX_CODE = '''
def escape_count(z: complex, c: complex) -> int:
    """Number of iterations before `|z| > 2`, capped at `max_iter`."""
    for i in range(max_iter):
        if abs(z) > 2:
            return i
        z = z * z + c
    return max_iter


def render(c: complex, width: int, height: int) -> list[int]:
    """Escape counts for the square `[-1.5, 1.5]`, row-major from the top left."""
    counts: list[int] = []
    for row in range(height):
        im = 1.5 - 3.0 * row / max(height - 1, 1)
        for col in range(width):
            re = -1.5 + 3.0 * col / max(width - 1, 1)
            counts.append(escape_count(complex(re, im), c))
    return counts


render(c, width, height)
'''


def palette() -> list[int]:
    """Flat `[r, g, b, ...]` palette: a blue-to-white ramp for escaping points, black for the set itself."""
    colours: list[int] = []
    for n in range(MAX_ITER):
        # Most points escape within a few iterations, so compress the low end to spread the colours out.
        t = (n / MAX_ITER) ** 0.35
        colours += [int(255 * t), int(255 * t**2), int(80 + 175 * t)]
    colours += [0, 0, 0]
    return colours


def julia_png(c: complex, width: int, height: int, out: Path) -> None:
    """Run the render in Monty and save the palette-indexed result at `out`."""
    # The sandbox holds `width * height` ints, so bound its memory and time rather than trusting the dimensions.
    limits: pydantic_monty.ResourceLimits = {'max_memory': 512 * 1024 * 1024, 'max_feed_duration_secs': 120}
    with pydantic_monty.Monty() as pool, pool.checkout(limits=limits) as session:
        start = time.perf_counter()
        counts: list[int] = session.feed_run(
            SANDBOX_CODE,
            inputs={'c': c, 'width': width, 'height': height, 'max_iter': MAX_ITER},
        )
        print(f'sandbox rendered {width}x{height} in {time.perf_counter() - start:.2f}s')

    # Mode 'P' stores one palette index per pixel, so the escape counts are the image data.
    image = Image.frombytes('P', (width, height), bytes(counts))
    image.putpalette(palette())
    image.save(out)
    print(f'wrote {out}')


def positive_int(text: str) -> int:
    """`argparse` type for a pixel dimension: an int of at least 1."""
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError(f'{text!r} is not a positive integer')
    return value


def main() -> None:
    parser = argparse.ArgumentParser(description='Render a Julia set in Monty and save it as a PNG.')
    parser.add_argument('out', nargs='?', type=Path, default=Path('julia.png'), help='output file (default: julia.png)')
    parser.add_argument('--c', nargs=2, type=float, default=(-0.7, 0.27015), metavar=('RE', 'IM'), help='the constant c')
    parser.add_argument('--size', nargs=2, type=positive_int, default=(800, 800), metavar=('WIDTH', 'HEIGHT'))
    args = parser.parse_args()
    julia_png(complex(*args.c), args.size[0], args.size[1], args.out)


if __name__ == '__main__':
    main()
