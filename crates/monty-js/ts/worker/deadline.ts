/** A cancellable wall-clock deadline, safe beyond `setTimeout`'s 32-bit cap. */
export interface DeadlineTimer {
  cancel(): void
}

/** Arms a deadline in milliseconds, chaining timers when it exceeds 2^31-1. */
export function deadlineTimer(timeoutMs: number, callback: () => void): DeadlineTimer {
  const deadline = Date.now() + timeoutMs
  let timer: ReturnType<typeof setTimeout> | null = null
  let cancelled = false
  const arm = () => {
    if (cancelled) return
    const remaining = deadline - Date.now()
    if (remaining <= 0) {
      timer = setTimeout(callback, 0)
    } else {
      timer = setTimeout(arm, Math.min(remaining, 2_147_483_647))
    }
  }
  arm()
  return {
    cancel() {
      cancelled = true
      if (timer !== null) clearTimeout(timer)
    },
  }
}
