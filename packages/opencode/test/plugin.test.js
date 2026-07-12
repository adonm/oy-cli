import assert from "node:assert/strict"
import test from "node:test"

import plugin from "../src/index.js"

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
