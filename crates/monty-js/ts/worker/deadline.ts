/** A cancellable deadline which does not overflow the host's signed 32-bit timer. */
export interface DeadlineTimer {
  cancel(): void
}

/** Arms a monotonic deadline, splitting long waits into supported timer intervals. */
export function deadlineTimer(ms: number, expired: () => void): DeadlineTimer {
  const deadline = performance.now() + ms
  let timer: ReturnType<typeof setTimeout>
  const tick = () => {
    const remaining = deadline - performance.now()
    if (remaining <= 0) expired()
    else timer = setTimeout(tick, Math.min(remaining, 0x7fffffff))
  }
  timer = setTimeout(tick, Math.min(ms, 0x7fffffff))
  return { cancel: () => clearTimeout(timer) }
}

/** Validates a timeout without silently disabling or overflowing its watchdog. */
export function timeoutOption(value: number | undefined, name: string): number | undefined {
  if (value !== undefined && (!Number.isFinite(value) || value < 0)) {
    throw new TypeError(`${name} must be a finite non-negative number`)
  }
  return value
}
