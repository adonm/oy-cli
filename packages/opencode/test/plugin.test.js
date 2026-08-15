import assert from "node:assert/strict"
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

import plugin, {
  applyCursorEnvironmentDefaults,
  createPlugin,
  defaultCursorStallMs,
  isGeneratedCursorRule,
  removeGeneratedCursorRule,
} from "../index.js"

const fakeBridge = async ({ onModels }) => ({
  url: "http://127.0.0.1:12345/v1",
  token: "test-token",
  current: { onModels },
  close() {},
})

const createEventStream = () => {
  const queued = []
  const waiting = []
  let closed = false
  return {
    push(event) {
      const resolve = waiting.shift()
      if (resolve) resolve(event)
      else queued.push(event)
    },
    subscribe({ signal }) {
      return {
        async *[Symbol.asyncIterator]() {
          signal.addEventListener(
            "abort",
            () => {
              closed = true
              for (const resolve of waiting.splice(0)) resolve(undefined)
            },
            { once: true },
          )
          while (!closed && !signal.aborted) {
            const event =
              queued.shift() ??
              (await new Promise((resolve) => {
                waiting.push(resolve)
              }))
            if (event === undefined) return
            yield event
          }
        },
      }
    },
  }
}

const createHarness = ({ onReload, legacySkills = false } = {}) => {
  const sources = []
  const skills = new Map()
  const agents = new Map()
  const commands = new Map()
  const integrations = new Map()
  const methods = []
  const providers = new Map()
  const models = new Map()
  const hooks = new Map()
  const events = createEventStream()
  let connection
  let credential
  let catalogTransform
  let reloads = 0
  const update = (entries) => (name, apply) => {
    const draft = {}
    apply(draft)
    entries.set(name, draft)
  }
  const catalog = {
    provider: { update: update(providers) },
    model: { update: (provider, model, mutate) => update(models)(`${provider}/${model}`, mutate) },
  }
  const replayCatalog = async () => {
    providers.clear()
    models.clear()
    await catalogTransform(catalog)
  }
  const skillDraft = legacySkills
    ? { source: (source) => sources.push(source) }
    : {
        list: () => Array.from(skills.values()),
        add: (skill) => skills.set(skill.id, { ...skill }),
        update: (name, mutate) => {
          const current = skills.get(name)
          if (!current) return
          mutate(current)
          current.id = name
        },
        remove: (name) => skills.delete(name),
        source: (source) => sources.push(source),
      }
  const ctx = {
    options: {},
    event: { subscribe: events.subscribe },
    skill: {
      transform: async (apply) => apply(skillDraft),
    },
    agent: {
      transform: async (apply) => apply({ update: update(agents) }),
    },
    command: {
      transform: async (apply) => apply({ update: update(commands) }),
    },
    integration: {
      transform: async (apply) =>
        apply({
          update: update(integrations),
          method: { update: (method) => methods.push(method) },
        }),
      connection: {
        active: async () => connection,
        resolve: async () => credential,
      },
    },
    catalog: {
      model: {
        list: async () => ({ location: { directory: process.cwd() }, data: [] }),
      },
      reload: async () => {
        reloads += 1
        await replayCatalog()
        onReload?.()
      },
      transform: async (apply) => {
        catalogTransform = apply
        await replayCatalog()
      },
    },
    aisdk: {
      sdk: async (hook) => hooks.set("sdk", hook),
      hook: async (name, hook) => hooks.set(name, hook),
    },
  }
  return {
    agents,
    commands,
    ctx,
    events,
    hooks,
    integrations,
    methods,
    models,
    providers,
    reloads: () => reloads,
    setConnection(value, resolved) {
      connection = value
      credential = resolved
    },
    skills,
    sources,
  }
}

test("registers the oy agent, skills, commands, and Cursor provider", async () => {
  const harness = createHarness()
  const environment = {}
  const deterministicPlugin = createPlugin({
    createCursor: () => ({ languageModel: () => ({}) }),
    environment,
    reportError: assert.fail,
    startCursorBridge: fakeBridge,
  })
  const cleanup = await deterministicPlugin.setup(harness.ctx)

  assert.equal(plugin.id, "oy")
  assert.equal(harness.sources.length, 0)
  for (const [id, expectedName] of [
    ["oy-audit", "oy-audit"],
    ["oy-review", "oy-review"],
    ["oy-enhance", "oy-enhance"],
  ]) {
    const skill = harness.skills.get(id)
    assert.ok(skill, `registers the ${id} skill`)
    assert.equal(skill.name, expectedName)
    assert.equal(typeof skill.description, "string")
    assert.ok(skill.description.length > 0)
    assert.equal(skill.slash, false)
    const source = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "../assets/skills", id, "SKILL.md"),
      "utf8",
    )
    const body = source.match(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)([\s\S]*)$/)?.[1].trim()
    assert.equal(skill.content, body)
  }
  assert.equal(harness.agents.get("oy").mode, "primary")
  assert.match(harness.agents.get("oy").system, /OpenCode and the user own permissions/)
  assert.deepEqual([...harness.commands.keys()], ["oy-audit", "oy-review", "oy-enhance"])
  assert.equal(harness.commands.get("oy-audit").agent, "oy")
  assert.equal(harness.integrations.get("cursor").name, "Cursor")
  assert.deepEqual(
    harness.methods.map((entry) => entry.method),
    [
      { type: "key", label: "Cursor API key" },
      { type: "env", names: ["CURSOR_API_KEY"] },
    ],
  )
  assert.deepEqual(
    harness.methods.map((entry) => entry.integrationID),
    ["cursor", "cursor"],
  )
  assert.deepEqual(harness.providers.get("cursor").api, {
    type: "aisdk",
    package: "@ai-sdk/openai",
    url: "http://127.0.0.1:12345/v1",
  })
  assert.equal(harness.providers.get("cursor").settings.baseURL, "http://127.0.0.1:12345/v1")
  assert.equal(harness.providers.get("cursor").settings.headers["x-oy-cursor-bridge"], "test-token")
  assert.deepEqual(harness.models.get("cursor/composer-2.5").api, {
    id: "composer-2.5",
    type: "aisdk",
    package: "@ai-sdk/openai",
    url: "http://127.0.0.1:12345/v1",
  })
  assert.deepEqual(
    harness.models.get("cursor/composer-2.5").variants.map((variant) => variant.id),
    ["off", "on", "plan"],
  )
  assert.equal(defaultCursorStallMs, 120_000)
  assert.equal(environment.OPENCODE_CURSOR_STALL_MS, String(defaultCursorStallMs))
  await cleanup()
})

test("preserves an explicit Cursor stall timeout", () => {
  const environment = { OPENCODE_CURSOR_STALL_MS: "0" }
  applyCursorEnvironmentDefaults(environment)
  assert.equal(environment.OPENCODE_CURSOR_STALL_MS, "0")
})

test("refreshes fallback models after a Cursor connection update", async () => {
  let resolveReload
  const reloaded = new Promise((resolve) => {
    resolveReload = resolve
  })
  const harness = createHarness({ onReload: resolveReload })
  const apiKeys = []
  const deterministicPlugin = createPlugin({
    createCursor: () => ({ languageModel: () => ({}) }),
    listCursorModels: async (apiKey) => {
      apiKeys.push(apiKey)
      return [{ id: "live-model", displayName: "Live Cursor model" }]
    },
    reportError: assert.fail,
    startCursorBridge: fakeBridge,
  })
  const cleanup = await deterministicPlugin.setup(harness.ctx)
  assert.equal(harness.models.has("cursor/composer-2.5"), true)
  await new Promise((resolve) => setImmediate(resolve))

  harness.setConnection({ id: "cursor-connection" }, { type: "key", key: "secret" })
  harness.events.push({
    type: "integration.connection.updated",
    data: { integrationID: "cursor" },
  })
  await reloaded

  for (let attempt = 0; attempt < 20 && apiKeys.length === 0; attempt += 1) {
    await new Promise((resolve) => setImmediate(resolve))
  }
  assert.deepEqual(apiKeys, ["secret"])
  assert.equal(harness.reloads(), 2)
  assert.equal(harness.models.has("cursor/composer-2.5"), false)
  assert.equal(harness.models.get("cursor/live-model").name, "Live Cursor model")
  await cleanup()
})

test("reruns model discovery with credentials updated during an active refresh", async () => {
  let resolveOld
  const oldModels = new Promise((resolve) => {
    resolveOld = resolve
  })
  let resolveNewReload
  const newReloaded = new Promise((resolve) => {
    resolveNewReload = resolve
  })
  const harness = createHarness({
    onReload: () => {
      if (harness.models.has("cursor/new-model")) resolveNewReload()
    },
  })
  const apiKeys = []
  const deterministicPlugin = createPlugin({
    createCursor: () => ({ languageModel: () => ({}) }),
    listCursorModels: async (apiKey) => {
      apiKeys.push(apiKey)
      if (apiKey === "old-key") return oldModels
      return [{ id: "new-model", displayName: "New credential model" }]
    },
    reportError: assert.fail,
    startCursorBridge: fakeBridge,
  })
  const cleanup = await deterministicPlugin.setup(harness.ctx)
  await new Promise((resolve) => setImmediate(resolve))

  harness.setConnection({ id: "cursor-connection" }, { type: "key", key: "old-key" })
  harness.events.push({
    type: "integration.connection.updated",
    data: { integrationID: "cursor" },
  })
  for (let attempt = 0; attempt < 20 && apiKeys.length === 0; attempt += 1) {
    await new Promise((resolve) => setImmediate(resolve))
  }

  harness.setConnection({ id: "cursor-connection" }, { type: "key", key: "new-key" })
  harness.events.push({
    type: "integration.connection.updated",
    data: { integrationID: "cursor" },
  })
  await new Promise((resolve) => setImmediate(resolve))
  resolveOld([{ id: "old-model", displayName: "Stale credential model" }])
  await newReloaded

  assert.deepEqual(apiKeys, ["old-key", "new-key"])
  assert.equal(harness.models.has("cursor/old-model"), false)
  assert.equal(harness.models.get("cursor/new-model").name, "New credential model")
  await cleanup()
})

test("falls back to directory skill sources on legacy hosts without add()", async () => {
  const harness = createHarness({ legacySkills: true })
  const deterministicPlugin = createPlugin({
    createCursor: () => ({ languageModel: () => ({}) }),
    reportError: assert.fail,
    startCursorBridge: fakeBridge,
  })
  const cleanup = await deterministicPlugin.setup(harness.ctx)

  assert.equal(harness.skills.size, 0)
  assert.equal(harness.sources.length, 1)
  assert.equal(harness.sources[0].type, "directory")
  assert.match(harness.sources[0].path, /assets[/\\]skills$/)
  await cleanup()
})

test("loads without the legacy event subscription surface", async () => {
  const harness = createHarness()
  delete harness.ctx.event
  const deterministicPlugin = createPlugin({
    createCursor: () => ({ languageModel: () => ({}) }),
    reportError: assert.fail,
    startCursorBridge: fakeBridge,
  })

  const cleanup = await deterministicPlugin.setup(harness.ctx)

  assert.equal(harness.models.has("cursor/composer-2.5"), true)
  await cleanup()
})

test("removes only Cursor's generated rule", () => {
  const directory = mkdtempSync(join(process.cwd(), ".cursor-rule-test-"))
  const path = join(directory, ".cursor", "rules", "opencode.mdc")
  mkdirSync(dirname(path), { recursive: true })
  try {
    const generated = "---\nalwaysApply: true\ngenerated: opencode-cursor\n---\n\nGenerated\n"
    writeFileSync(path, generated)
    assert.equal(isGeneratedCursorRule(generated), true)
    removeGeneratedCursorRule(directory)
    assert.equal(existsSync(path), false)

    const userRule = "---\nalwaysApply: true\n---\n\nMentions generated: opencode-cursor\n"
    writeFileSync(path, userRule)
    assert.equal(isGeneratedCursorRule(userRule), false)
    removeGeneratedCursorRule(directory)
    assert.equal(readFileSync(path, "utf8"), userRule)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("refuses symlinked Cursor rule ancestors during cleanup", () => {
  const generated = "---\nalwaysApply: true\ngenerated: opencode-cursor\n---\n\nGenerated\n"
  const cases = [
    { link: [".cursor"], escaped: ["rules", "opencode.mdc"] },
    { link: [".cursor", "rules"], escaped: ["opencode.mdc"] },
  ]
  for (const scenario of cases) {
    const directory = mkdtempSync(join(process.cwd(), ".cursor-rule-boundary-test-"))
    const outside = mkdtempSync(join(process.cwd(), ".cursor-rule-outside-test-"))
    const link = join(directory, ...scenario.link)
    const escaped = join(outside, ...scenario.escaped)
    mkdirSync(dirname(link), { recursive: true })
    mkdirSync(dirname(escaped), { recursive: true })
    writeFileSync(escaped, generated)
    symlinkSync(outside, link, "dir")
    try {
      removeGeneratedCursorRule(directory)
      assert.equal(readFileSync(escaped, "utf8"), generated)
    } finally {
      rmSync(directory, { recursive: true, force: true })
      rmSync(outside, { recursive: true, force: true })
    }
  }
})

test("refuses a symlinked generated Cursor rule", () => {
  const directory = mkdtempSync(join(process.cwd(), ".cursor-rule-link-test-"))
  const outside = mkdtempSync(join(process.cwd(), ".cursor-rule-link-outside-test-"))
  const path = join(directory, ".cursor", "rules", "opencode.mdc")
  const escaped = join(outside, "opencode.mdc")
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(escaped, "---\ngenerated: opencode-cursor\n---\n")
  symlinkSync(escaped, path)
  try {
    removeGeneratedCursorRule(directory)
    assert.equal(existsSync(path), true)
  } finally {
    rmSync(directory, { recursive: true, force: true })
    rmSync(outside, { recursive: true, force: true })
  }
})

test("installed plugin layout resolves packaged assets and modules", async () => {
  const packageDirectory = fileURLToPath(new URL("../", import.meta.url))
  const directory = mkdtempSync(join(packageDirectory, ".plugin-layout-test-"))
  try {
    cpSync(join(packageDirectory, "assets"), join(directory, "assets"), { recursive: true })
    cpSync(join(packageDirectory, "index.js"), join(directory, "index.js"))
    cpSync(join(packageDirectory, "cursor-catalog.js"), join(directory, "cursor-catalog.js"))
    cpSync(join(packageDirectory, "cursor-bridge.js"), join(directory, "cursor-bridge.js"))
    cpSync(join(packageDirectory, "cursor-paths.js"), join(directory, "cursor-paths.js"))
    cpSync(join(packageDirectory, "cursor-provider.js"), join(directory, "cursor-provider.js"))
    cpSync(join(packageDirectory, "cursor-skills.js"), join(directory, "cursor-skills.js"))
    cpSync(join(packageDirectory, "vendor"), join(directory, "vendor"), { recursive: true })
    const installed = await import(`${pathToFileURL(join(directory, "index.js")).href}?test=${Date.now()}`)

    assert.equal(installed.default.id, "oy")
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
