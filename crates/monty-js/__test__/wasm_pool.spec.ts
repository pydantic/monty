import { kind } from './env.js'
import { runPoolConformanceTests } from './poolConformance.js'

import { Monty } from '@pydantic/monty/wasm'

runPoolConformanceTests(`${kind} wasm pool`, (options) => Monty.create(options), {
  timeoutExitStatus: kind === 'node',
})
