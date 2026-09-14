"""The flight physics of xkcd 353, computed inside the Monty sandbox.

The sandbox has no DOM and no `random`, so `move()` returns the offset for one
tick for the host to draw, and the generator below stands in for `random`.
See the README for what else the port changes.
"""

import math
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    # The host binds `seed` as an input before this module runs; the value here
    # is only so that type checkers see a bound name.
    seed: int = 0


class Rng:
    """A 31-bit linear congruential generator, standing in for `random`.

    Monty has no `random` module, and nothing inside the sandbox is a source
    of entropy, so the seed has to arrive from the host.  The multiplier and
    modulus are MINSTD's; the state stays well inside 64 bits.
    """

    def __init__(self, seed: int) -> None:
        self.state = seed % 2147483647 or 42

    def random(self) -> float:
        """The next float in (0, 1), as `random.random()` would give it."""
        self.state = self.state * 48271 % 2147483647
        return self.state / 2147483647

    def normalvariate(self, mu: float, sigma: float) -> float:
        """A normally distributed float, via the Box-Muller transform."""
        radius = math.sqrt(-2 * math.log(self.random()))
        angle = 2 * math.pi * self.random()
        return mu + sigma * radius * math.cos(angle)


class Antigravity:
    """The flight path: drift sideways, climb to 50, then wander upwards."""

    def __init__(self, seed: int) -> None:
        self.rng = Rng(seed)
        self.xoffset = 0.0
        self.yoffset = 0.0

    def move(self) -> tuple[float, float]:
        """Advances one tick and returns the offset for the host to apply."""
        offset = (self.xoffset, -self.yoffset)
        self.xoffset += self.rng.normalvariate(0, 1) / 20
        if self.yoffset < 50:
            self.yoffset += 0.1
        else:
            self.yoffset += self.rng.normalvariate(0, 1) / 20
        return offset


# The host ticks `flight` once per frame.
flight = Antigravity(seed)
