// Exercise binary resolution through the installed platform package, not a CI override.
delete process.env.MONTY_BIN

const { Monty } = await import('@pydantic/monty')
const pool = await Monty.create()
const session = await pool.checkout()
const result = await session.feedRun('6 * 7')
await session.close()
await pool.close()

if (result !== 42) throw new Error(`expected 42, got ${result}`)
