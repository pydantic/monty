"""Fly the pydantic/thrust rocket lander from launch pad to landing pad: the control-loop case.

The game's physics, world generation and wind are ported here from `src/physics.ts`,
`world.ts`, `wind.ts` and `random.ts` in github.com/pydantic/thrust, seeded so every
attempt flies the same map. The task prompt is that repo's default pilot instructions
(`DEFAULT_PILOT_INSTRUCTIONS` in `server/thrust_server/agent.py`, the code default for
its Logfire `pilot-instructions` prompt), adapted to plain dicts: the real game hands
the script dataclasses through `ClassType`, this task hands it dicts and lists.

A flight is hundreds of sequential `await update(move)` calls, each one a host round
trip, so `call_batches` equals `external_calls` by construction and the score is the
landing time.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any, cast

from evals.harness.evaluators import Predicate
from evals.harness.task import Task

PHYSICS = {
    'dt': 1 / 60,
    'thrust_accel': 9.0,
    'rotation_accel': 6.0,
    'angular_damping': 2.5,
    'max_angular_velocity': 3.0,
    'wind_drag': 0.15,
    'rocket_height': 4.0,
    'rocket_half_base': 1.25,
    'landing_max_angle': 0.28,
    'landing_max_vy': 5.0,
    'landing_max_vx': 3.2,
    'max_flight_time': 90.0,
    'control_ticks': 6,
}
WORLD_HEIGHT = 100.0
GRAVITY = 4.0
MAX_WORLD_WIDTH = 400.0
PAD_WIDTH = 12.0
LAUNCH_PAD_WIDTH = 10.0
MIN_HEIGHT, MAX_HEIGHT = 5.0, 60.0
MEAN_HEIGHT, MEAN_REVERSION = 22.0, 0.15
SAMPLE_SPACING = 4
PAD_CANDIDATES = 8
MIN_PAD_SEPARATION = 0.4
SPAWN_MARGIN = 15.0
MIN_BASE_SPEED, MAX_BASE_SPEED, GUST_FRACTION = 1.5, 7.0, 0.6


class Mulberry32:
    """mulberry32, matching the TypeScript bit for bit so seeds replay the same world."""

    def __init__(self, seed: int) -> None:
        self.a = seed & 0xFFFFFFFF

    def next(self) -> float:
        self.a = (self.a + 0x6D2B79F5) & 0xFFFFFFFF
        t = self.a
        t = (_imul(t ^ (t >> 15), t | 1)) & 0xFFFFFFFF
        t ^= (t + _imul(t ^ (t >> 7), t | 61)) & 0xFFFFFFFF
        return ((t ^ (t >> 14)) & 0xFFFFFFFF) / 4294967296

    def range(self, lo: float, hi: float) -> float:
        return lo + (hi - lo) * self.next()


def _imul(a: int, b: int) -> int:
    """JavaScript `Math.imul`: 32-bit signed multiply, returned as an unsigned 32-bit value."""
    r = (a * b) & 0xFFFFFFFF
    return r


def _clamp(v: float, lo: float, hi: float) -> float:
    return min(hi, max(lo, v))


def terrain_height_at(terrain: list[tuple[float, float]], x: float) -> float:
    if x <= terrain[0][0]:
        return terrain[0][1]
    if x >= terrain[-1][0]:
        return terrain[-1][1]
    lo, hi = 0, len(terrain) - 1
    while hi - lo > 1:
        mid = (lo + hi) >> 1
        if terrain[mid][0] <= x:
            lo = mid
        else:
            hi = mid
    (x0, y0), (x1, y1) = terrain[lo], terrain[hi]
    t = 0 if x1 == x0 else (x - x0) / (x1 - x0)
    return y0 + (y1 - y0) * t


@dataclass
class Pad:
    x1: float
    x2: float
    y: float


@dataclass
class World:
    width: float
    terrain: list[tuple[float, float]]
    pad: Pad
    launch_pad: Pad


def generate_world(rng: Mulberry32, width: float) -> World:
    max_samples = int(MAX_WORLD_WIDTH // SAMPLE_SPACING) + 1
    h = rng.range(8, 30)
    raw: list[float] = []
    for _ in range(max_samples):
        pull = (MEAN_HEIGHT - h) * MEAN_REVERSION
        h = _clamp(h + pull + rng.range(-13, 13), MIN_HEIGHT, MAX_HEIGHT)
        raw.append(h)
    smoothed: list[float] = []
    for i, v in enumerate(raw):
        prev = raw[i - 1] if i > 0 else v
        nxt = raw[i + 1] if i + 1 < len(raw) else v
        smoothed.append((prev + v + nxt) / 3)
    xs: list[float] = []
    heights: list[float] = []
    for i in range(max_samples):
        x = i * SAMPLE_SPACING
        if x >= width:
            break
        xs.append(x)
        heights.append(smoothed[i])
    all_pts = [(float(i * SAMPLE_SPACING), y) for i, y in enumerate(smoothed)]
    xs.append(width)
    heights.append(terrain_height_at(all_pts, width))

    min_sep = MIN_PAD_SEPARATION * width
    pad_min = 4 + PAD_WIDTH / 2
    pad_max = width - 4 - PAD_WIDTH / 2

    def allowed(spawn: float) -> list[tuple[float, float]]:
        return [(a, b) for a, b in ((pad_min, spawn - min_sep), (spawn + min_sep, pad_max)) if b > a]

    spawn_x = SPAWN_MARGIN + rng.next() * (width - 2 * SPAWN_MARGIN)
    ranges = allowed(spawn_x)
    if not ranges:
        spawn_x = SPAWN_MARGIN if rng.next() < 0.5 else width - SPAWN_MARGIN
        ranges = allowed(spawn_x)
    samples = list(zip(xs, heights))
    launch = Pad(
        spawn_x - LAUNCH_PAD_WIDTH / 2,
        spawn_x - LAUNCH_PAD_WIDTH / 2 + LAUNCH_PAD_WIDTH,
        terrain_height_at(samples, spawn_x),
    )
    total = sum(b - a for a, b in ranges)
    centre = pad_max if spawn_x < width / 2 else pad_min
    pad_h = terrain_height_at(samples, centre)
    for _ in range(PAD_CANDIDATES):
        if total <= 0:
            break
        offset = rng.next() * total
        c = centre
        for a, b in ranges:
            if offset <= b - a:
                c = a + offset
                break
            offset -= b - a
        hc = terrain_height_at(samples, c)
        if hc < pad_h:
            pad_h, centre = hc, c
    pad = Pad(centre - PAD_WIDTH / 2, centre + PAD_WIDTH / 2, pad_h)
    return World(width, _with_flats(xs, heights, [launch, pad]), pad, launch)


def _with_flats(xs: list[float], heights: list[float], flats: list[Pad]) -> list[tuple[float, float]]:
    flats = sorted(flats, key=lambda f: f.x1)
    out: list[tuple[float, float]] = []
    fi = 0
    for x, y in zip(xs, heights):
        while fi < len(flats) and flats[fi].x2 < x:
            out += [(flats[fi].x1, flats[fi].y), (flats[fi].x2, flats[fi].y)]
            fi += 1
        if fi < len(flats) and flats[fi].x1 <= x <= flats[fi].x2:
            continue
        out.append((x, y))
    while fi < len(flats):
        out += [(flats[fi].x1, flats[fi].y), (flats[fi].x2, flats[fi].y)]
        fi += 1
    return out


@dataclass
class Wind:
    base_x: float
    base_y: float
    phases: tuple[float, float, float]

    def at(self, x: float, y: float, t: float) -> tuple[float, float]:
        p0, p1, p2 = self.phases
        speed = math.hypot(self.base_x, self.base_y)
        scale = GUST_FRACTION * speed
        gx = (
            math.sin(y * 0.12 + t * 0.6 + p0) * 0.5
            + math.sin(x * 0.07 - t * 0.45 + p1) * 0.3
            + math.sin((x - y) * 0.05 + t * 0.9 + p2) * 0.2
        )
        gy = (
            math.cos(x * 0.12 - t * 0.6 + p0) * 0.5
            + math.sin(y * 0.09 + t * 0.7 + p2) * 0.3
            + math.cos((x + y) * 0.06 - t * 0.5 + p1) * 0.2
        )
        return self.base_x + gx * scale, self.base_y + gy * scale * 0.8


def generate_wind(rng: Mulberry32) -> Wind:
    speed = rng.range(MIN_BASE_SPEED, MAX_BASE_SPEED)
    direction = -1 if rng.next() < 0.5 else 1
    vertical = rng.range(-0.25, 0.25)
    return Wind(
        direction * speed, speed * vertical, (rng.range(0, math.tau), rng.range(0, math.tau), rng.range(0, math.tau))
    )


def _on_flat(flat: Pad, base: list[tuple[float, float]]) -> bool:
    """Both base corners rest within the flat's x range."""
    return all(flat.x1 <= cx <= flat.x2 for cx, _ in base)


def _normalise(a: float) -> float:
    r = math.fmod(a, math.tau)
    if r > math.pi:
        r -= math.tau
    if r < -math.pi:
        r += math.tau
    return r


class Game:
    """One flight: the rocket state, stepped `control_ticks` physics ticks per `update`."""

    def __init__(self, seed: int, width: float) -> None:
        rng = Mulberry32(seed)
        self.world = generate_world(rng, width)
        self.wind = generate_wind(rng)
        lp = self.world.launch_pad
        self.x = (lp.x1 + lp.x2) / 2
        self.y = lp.y + PHYSICS['rocket_height'] * 0.4
        self.vx = self.vy = 0.0
        self.angle = 0.0
        self.angular_velocity = 0.0
        self.status = 'flying'
        self.tick = 0
        self.time = 0.0
        self.wind_at = self.wind.at(self.x, self.y, 0.0)

    def corners(self) -> list[tuple[float, float]]:
        h, hb = PHYSICS['rocket_height'], PHYSICS['rocket_half_base']
        c, s = math.cos(self.angle), math.sin(self.angle)
        out: list[tuple[float, float]] = []
        for px, py in ((0.0, h * 0.6), (hb, -h * 0.4), (-hb, -h * 0.4)):
            out.append((self.x + px * c + py * s, self.y - px * s + py * c))
        return out

    def step(self, thrust: bool, left: bool, right: bool) -> None:
        if self.status != 'flying':
            return
        p = PHYSICS
        dt = p['dt']
        torque = (p['rotation_accel'] if right else 0.0) - (p['rotation_accel'] if left else 0.0)
        self.angular_velocity += (torque - p['angular_damping'] * self.angular_velocity) * dt
        self.angular_velocity = _clamp(self.angular_velocity, -p['max_angular_velocity'], p['max_angular_velocity'])
        self.angle += self.angular_velocity * dt
        wx, wy = self.wind.at(self.x, self.y, self.time)
        self.wind_at = (wx, wy)
        ax = p['wind_drag'] * (wx - self.vx)
        ay = -GRAVITY + p['wind_drag'] * (wy - self.vy)
        if thrust:
            ax += math.sin(self.angle) * p['thrust_accel']
            ay += math.cos(self.angle) * p['thrust_accel']
        prev_x = self.x
        self.vx += ax * dt
        self.vy += ay * dt
        self.x += self.vx * dt
        self.y += self.vy * dt
        corners = self.corners()
        w = self.world
        if any(cx < 0 or cx > w.width or cy > WORLD_HEIGHT for cx, cy in corners):
            self.status = 'crashed'
            return
        touching = any(cy <= terrain_height_at(w.terrain, cx) for cx, cy in corners)
        if not touching:
            return
        base = corners[1:]
        gentle = (
            abs(_normalise(self.angle)) < p['landing_max_angle']
            and abs(self.vy) < p['landing_max_vy']
            and abs(self.vx) < p['landing_max_vx']
        )
        if _on_flat(w.pad, base) and gentle:
            self.status = 'landed'
            self.y = w.pad.y + p['rocket_height'] * 0.4
            self.vx = self.vy = self.angular_velocity = self.angle = 0.0
            return
        if _on_flat(w.launch_pad, base) and gentle:
            self.x = prev_x
            self.y = max(self.y, w.launch_pad.y + p['rocket_height'] * 0.4)
            self.vx = 0.0
            self.vy = max(0.0, self.vy)
            return
        self.status = 'crashed'

    def update(self, thrust: bool, left: bool, right: bool) -> dict[str, Any]:
        for _ in range(int(PHYSICS['control_ticks'])):
            self.step(thrust, left, right)
            self.tick += 1
            if self.status == 'flying':
                self.time += PHYSICS['dt']
                if self.time >= PHYSICS['max_flight_time']:
                    self.status = 'timeout'
            if self.status != 'flying':
                break
        return self.state()

    def state(self) -> dict[str, Any]:
        return {
            'status': self.status,
            'tick': self.tick,
            'time': self.tick * PHYSICS['dt'],
            'x': self.x,
            'y': self.y,
            'vx': self.vx,
            'vy': self.vy,
            'angle': self.angle,
            'angular_velocity': self.angular_velocity,
            'wind_x': self.wind_at[0],
            'wind_y': self.wind_at[1],
        }


SEED = 1
WIDTH = 160.0

_game = Game(SEED, WIDTH)


def _reset() -> None:
    """Start the flight again from the launch pad."""
    global _game
    _game = Game(SEED, WIDTH)


async def update(move: dict[str, bool]) -> dict[str, Any]:
    """Host function: hold `move` for `control_ticks` physics ticks and return the rocket."""
    if _game.status != 'flying':
        raise RuntimeError('the flight is over, stop calling update()')
    return _game.update(bool(move.get('thrust')), bool(move.get('left')), bool(move.get('right')))


def _inputs() -> dict[str, Any]:
    world = _game.world
    return {
        'status': _game.state(),
        'pad': {'x1': world.pad.x1, 'x2': world.pad.x2, 'y': world.pad.y},
        'launch_pad': {'x1': world.launch_pad.x1, 'x2': world.launch_pad.x2, 'y': world.launch_pad.y},
        'terrain': [list(point) for point in world.terrain],
        'world': {'width': world.width, 'height': WORLD_HEIGHT, 'gravity': GRAVITY},
        'physics': dict(PHYSICS),
    }


STUBS = '''
from typing import Any

status: dict[str, Any]
"""The rocket at the start, resting upright on the launch pad with v = 0. Keys as `update` returns."""
pad: dict[str, float]
"""The landing pad (the target): `x1`, `x2`, `y`."""
launch_pad: dict[str, float]
"""Where the rocket starts: `x1`, `x2`, `y`."""
terrain: list[list[float]]
"""Polyline `[x, y]` points, x strictly increasing from 0 to `world["width"]`."""
world: dict[str, float]
"""`width`, `height` and `gravity` (m/s^2, always pulling down)."""
physics: dict[str, float]
"""Simulation constants: `dt`, `thrust_accel`, `rotation_accel`, `angular_damping`,
`max_angular_velocity`, `wind_drag`, `rocket_height`, `rocket_half_base`, `landing_max_angle`,
`landing_max_vy`, `landing_max_vx`, `max_flight_time`, `control_ticks`."""

async def update(move: dict[str, bool]) -> dict[str, Any]:
    """Hold `move` for `physics["control_ticks"]` physics ticks and return the rocket afterwards.

    `move` has the keys `thrust`, `left` and `right`, each on or off; there is no throttle.
    The returned dict has `status` ("flying", "landed", "crashed" or "timeout"; over once
    not flying), `tick`, `time` (seconds since the start, the score), `x`, `y` (centre of
    the body, y up), `vx`, `vy`, `angle` (radians, 0 = nose up, positive = clockwise),
    `angular_velocity`, `wind_x` and `wind_y` (wind at the rocket, m/s).
    This is the only way to control the rocket and the only way simulated time advances.
    """
    ...
'''

PROMPT = """\
Write the autopilot for a 2D rocket lander game. The code flies one flight from start to
finish. You get one flight: there is no second attempt once the script is in the air, so
think the strategy through before you reply.

## How the script runs

The names `status`, `pad`, `launch_pad`, `terrain`, `world` and `physics` are predefined
dicts and lists describing this flight, and `update(move)` is the one host function; do
not redefine or shadow them.

- `await update(move)` holds the move for `physics["control_ticks"]` physics ticks of
  `physics["dt"]` seconds each (0.1 s in all), so that is your control interval. The code
  must keep calling it, computing the next move from the returned status, until the
  status is not "flying", and then finish with the final status dict as its last
  expression. A flight still going after `physics["max_flight_time"]` seconds ends with
  status "timeout", which counts as a failure like a crash. A flight is hundreds of
  updates, so keep the work per update small and never sleep or busy-wait.
- An uncaught exception ends the script; the rocket then drifts uncontrolled. Guard
  divisions and list indexing.

## The world

Units are metres and seconds, y is up, angles are radians with 0 = nose up and positive =
clockwise (nose to the right). `status["x"], status["y"]` is the centre of the body. The
body has three corners: the nose tip `0.6 * rocket_height` above the centre along the body
axis and two base corners `0.4 * rocket_height` below it, `rocket_half_base` either side.
The terrain is a polyline; the ground height at any x is the linear interpolation between
the two surrounding points (write a helper for it). Pads are flat parts of the terrain.
Leaving the world is a crash: the walls are hard, and any corner of the rocket touching
the left or right edge or the top (`y > world["height"]`) ends the flight. Mountains can
reach a good fraction of that height, so plan a cruising altitude that clears every peak
between you and the pad with margin while staying well below the top. The rocket starts
upright at rest on the launch pad, already "flying", and the clock is running.

## Physics, exactly as the game computes each tick

```
torque = rotation_accel * ((1 if right else 0) - (1 if left else 0))
angular_velocity += (torque - angular_damping * angular_velocity) * dt
angular_velocity = clamp(angular_velocity, -max_angular_velocity, max_angular_velocity)
angle += angular_velocity * dt
ax = wind_drag * (wind_x - vx) + (sin(angle) * thrust_accel if thrust else 0)
ay = wind_drag * (wind_y - vy) - gravity + (cos(angle) * thrust_accel if thrust else 0)
vx += ax * dt;  vy += ay * dt;  x += vx * dt;  y += vy * dt
```

The wind is a steady base of 1.5 to 7 m/s, mostly horizontal and never reversing, with
swirling gusts of up to 60% on top, so the local wind changes as you move and can reach
12 m/s. Read `wind_x`/`wind_y` every update rather than assuming a constant.

Thrust and rotation are on/off and held for a whole update, so hovering means pulsing
the engine on roughly `gravity / thrust_accel` of the updates, and a nose tilt of `angle`
gives a sideways acceleration of `thrust_accel * sin(angle)` while thrusting. Rotation
has inertia and damping: to hold an angle, steer the angular velocity toward
`k * (target - angle)` and apply left/right with a dead band, rather than flipping the
inputs every update.

## Goal

Take off from the launch pad, fly to the landing pad and land on it as quickly as you
can. Time to touchdown is the score, but a crash scores nothing. Landing means touching
the pad with both base corners on it, |angle| below `landing_max_angle`, |vy| below
`landing_max_vy` and |vx| below `landing_max_vx`.
"""

REFERENCE = """
import math

def ground_at(x):
    if x <= terrain[0][0]:
        return terrain[0][1]
    if x >= terrain[-1][0]:
        return terrain[-1][1]
    lo = 0
    hi = len(terrain) - 1
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if terrain[mid][0] <= x:
            lo = mid
        else:
            hi = mid
    x0 = terrain[lo][0]
    y0 = terrain[lo][1]
    x1 = terrain[hi][0]
    y1 = terrain[hi][1]
    t = 0.0 if x1 == x0 else (x - x0) / (x1 - x0)
    return y0 + (y1 - y0) * t

def clamp(v, lo, hi):
    return max(lo, min(hi, v))

pad_x = (pad['x1'] + pad['x2']) / 2
start_x = status['x']
lo_x = min(start_x, pad_x) - 6
hi_x = max(start_x, pad_x) + 6
peak = max([p[1] for p in terrain if lo_x <= p[0] <= hi_x])
cruise = min(peak + 14, world['height'] - 12)
thrust_accel = physics['thrust_accel']

st = status
phase = 'climb'
n = 0
while st['status'] == 'flying' and n < 2000:
    n += 1
    x = st['x']
    y = st['y']
    vx = st['vx']
    vy = st['vy']
    angle = st['angle']
    av = st['angular_velocity']
    dx = pad_x - x
    if phase == 'climb' and y >= cruise:
        phase = 'cruise'
    if phase == 'cruise' and abs(dx) < 3 and abs(vx) < 1.5:
        phase = 'descend'
    if phase == 'climb':
        target_vx = 0.0
        target_vy = 8.0
    elif phase == 'cruise':
        target_vx = clamp(dx * 0.6, -9.0, 9.0)
        target_vy = (cruise - y) * 0.5
    else:
        target_vx = clamp(dx * 0.8, -2.0, 2.0)
        alt = y - pad['y'] - 1.6
        target_vy = -min(4.0, 0.6 + alt * 0.25)
    ax_des = (target_vx - vx) * 0.8
    target_angle = clamp(math.asin(clamp(ax_des / thrust_accel, -0.9, 0.9)), -0.5, 0.5)
    if phase == 'descend' and abs(dx) < 1.0:
        target_angle = target_angle * 0.5
    want_av = clamp((target_angle - angle) * 6.0, -2.0, 2.0)
    left = False
    right = False
    if want_av - av > 0.15:
        right = True
    elif want_av - av < -0.15:
        left = True
    thrust = vy < target_vy
    if n % 10 == 0:
        print(f'{st["time"]:.1f}s {phase} x={x:.1f} y={y:.1f} vx={vx:.1f} vy={vy:.1f}')
    st = await update({'thrust': thrust, 'left': left, 'right': right})

st
"""


def _landed(result: Any) -> bool:
    """The final status is `landed` and the clock stopped before the flight timed out."""
    if not isinstance(result, dict):
        return False
    status = cast(dict[str, Any], result)
    return status.get('status') == 'landed' and float(status.get('time', 1e9)) < PHYSICS['max_flight_time']


TASK = Task(
    name='thrust',
    category='sims',
    prompt=PROMPT,
    stubs=STUBS,
    tools={'update': update},
    inputs=_inputs(),
    expected={'status': 'landed'},
    evaluators=(Predicate('landed inside max_flight_time', _landed),),
    reference_solution=REFERENCE,
    traps=('hundreds of sequential round trips', 'math.asin', 'f-string format specs in prints'),
    max_result_bytes=400,
    setup=_reset,
)
