export const kind = typeof window === 'undefined' ? 'node' : 'browser'

interface SkipContext {
  skip(): void
}

export function skipIfBrowser(ctx: SkipContext): void {
  if (kind === 'browser') {
    ctx.skip()
  }
}

export function skipIfNode(ctx: SkipContext): void {
  if (kind === 'node') {
    ctx.skip()
  }
}
