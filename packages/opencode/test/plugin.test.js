import assert from "node:assert/strict"
import { cp, mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { pathToFileURL } from "node:url"

import plugin from "../index.js"

test("registers the oy agent, skills, and commands", async () => {
  const sources = []
  const agents = new Map()
  const commands = new Map()
  const update = (entries) => (name, apply) => {
    const draft = {}
    apply(draft)
    entries.set(name, draft)
  }

  await plugin.setup({
    options: {},
    skill: {
      transform: async (apply) => apply({ source: (source) => sources.push(source) }),
    },
    agent: {
      transform: async (apply) => apply({ update: update(agents) }),
    },
    command: {
      transform: async (apply) => apply({ update: update(commands) }),
    },
  })

  assert.equal(plugin.id, "oy")
  assert.equal(sources.length, 1)
  assert.equal(sources[0].type, "directory")
  assert.match(sources[0].path, /assets[/\\]skills$/)
  assert.equal(agents.get("oy").mode, "primary")
  assert.match(agents.get("oy").system, /OpenCode and the user own permissions/)
  assert.deepEqual([...commands.keys()], ["oy-audit", "oy-review", "oy-enhance"])
  assert.equal(commands.get("oy-audit").agent, "oy")
})

test("installed plugin layout resolves its assets", async () => {
  const dir = await mkdtemp(join(tmpdir(), "oy-plugin-layout-"))
  try {
    const pluginsDir = join(dir, "plugins")
    await cp("assets", join(pluginsDir, "assets"), { recursive: true })
    await cp("index.js", join(pluginsDir, "oy.js"))
    const installed = await import(pathToFileURL(join(pluginsDir, "oy.js")))
    assert.equal(installed.default.id, "oy")
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})
