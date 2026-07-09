import { Monty } from '@pydantic/monty'
import { kind } from './env.js'
import { runMontyApiTests } from '../test-support/publicApi.js'

runMontyApiTests(`${kind} public API`, () => Monty.create())
