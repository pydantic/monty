import { expect, test } from 'vitest'

import { componentAssetSources } from '../scripts/component-assets.ts'

// Exercise counts on both sides of the four/five-module layouts emitted by past Jco builds.
test.each([1, 4, 6])('generates asset names and literal browser URLs for %i core modules', (count) => {
  const coreModules = Array.from({ length: count }, (_, index) => `monty.component.core${index + 1}.wasm`)
  const sources = componentAssetSources(coreModules)
  const names = sources['componentModules.ts'].match(/COMPONENT_MODULE_NAMES = (\[.*\]) as const/)?.[1]
  expect(JSON.parse(names ?? 'null')).toEqual(coreModules)

  const urls = Array.from(
    sources['loadModule.browser.ts'].matchAll(/new URL\((".*?"), import\.meta\.url\)/g),
    (match) => JSON.parse(match[1]),
  )
  expect(urls).toEqual(coreModules.map((name) => `./component/${name}`))
})
