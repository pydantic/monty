# thrust

Fly the `pydantic/thrust` rocket from the launch pad to the landing pad and land inside `max_flight_time`.
The physics, world generation and wind are ported host-side from that repo's `src/physics.ts`, `world.ts`, `wind.ts`
and `random.ts`, with seed 1 on a 160 m world; the prompt is the repo's default pilot instructions (the code default
of its Logfire `pilot-instructions` prompt) adapted to dicts.
The real game hands the script dataclasses through `ClassType`; here `status`, `pad`, `launch_pad`, `terrain`,
`world` and `physics` are plain dicts and lists.

`update(move)` is the one host function: it holds `thrust`/`left`/`right` for six physics ticks (0.1 s) and returns
the rocket's state, so a flight is hundreds of sequential round trips and `call_batches` equals `external_calls`.
`setup` restarts the flight.

Scored with one `Predicate`: the returned status is `landed` with `time` under `max_flight_time`.
The reference autopilot climbs above every peak between the pads, translates with a proportional angle controller
and dead band, then descends; it lands at 36.5 s.
