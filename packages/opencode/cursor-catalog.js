export const cursorProviderPackage = "@ai-sdk/openai"

export const fallbackCursorModels = [
  {
    id: "composer-2.5",
    displayName: "Composer 2.5",
    parameters: [{ id: "thinking", values: [{ value: "off" }, { value: "on" }] }],
  },
  { id: "claude-opus-4-8", displayName: "Claude Opus 4.8 (via Cursor)" },
  { id: "claude-sonnet-4-6", displayName: "Claude Sonnet 4.6 (via Cursor)" },
  { id: "gpt-5.5", displayName: "GPT-5.5 (via Cursor)" },
]

const reasoningParameter = /think|reason|effort/i
const booleanValues = new Set(["true", "false"])
const contextLimits = {
  "claude-fable-5": 300_000,
  "claude-opus-4-7": 300_000,
  "claude-opus-4-8": 300_000,
  "claude-opus-5": 300_000,
  "gpt-5-mini": 272_000,
  "gpt-5.1": 272_000,
  "gpt-5.2": 272_000,
  "gpt-5.3-codex": 272_000,
  "gpt-5.4": 272_000,
  "gpt-5.4-mini": 272_000,
  "gpt-5.4-nano": 272_000,
  "gpt-5.5": 272_000,
  "gpt-5.6-luna": 272_000,
  "gpt-5.6-sol": 272_000,
  "gpt-5.6-terra": 272_000,
  "grok-4.5": 256_000,
}
const outputLimits = {
  "claude-fable-5": 64_000,
  "claude-opus-4-7": 64_000,
  "claude-opus-4-8": 64_000,
  "claude-opus-5": 64_000,
  "gpt-5.5": 64_000,
  "gpt-5.6-sol": 64_000,
}
const noAutoCompactionInputLimit = 1_000_000_000
const modelCosts = {
  "claude-fable-5": [10, 50, 1, 12.5],
  "claude-haiku-4-5": [1, 5, 0.1, 1.25],
  "claude-opus-4": [5, 25, 0.5, 6.25],
  "claude-sonnet-4": [3, 15, 0.3, 3.75],
  "claude-sonnet-5": [3, 15, 0.3, 3.75],
  "gemini-2.5-flash": [0.3, 2.5, 0.03, 0],
  "gemini-3-flash": [0.5, 3, 0.05, 0],
  "gemini-3.1-pro": [2, 12, 0.2, 0],
  "gemini-3.5-flash": [1.5, 9, 0.15, 0],
  "gemini-3.6-flash": [1.5, 7.5, 0.15, 0],
  "glm-5.2": [1.4, 4.4, 0.26, 0],
  "gpt-5-mini": [0.25, 2, 0.025, 0],
  "gpt-5.1": [1.25, 10, 0.125, 0],
  "gpt-5.2": [1.75, 14, 0.175, 0],
  "gpt-5.3-codex": [1.75, 14, 0.175, 0],
  "gpt-5.4-mini": [0.75, 4.5, 0.075, 0],
  "gpt-5.4-nano": [0.2, 1.25, 0.02, 0],
  "gpt-5.4": [2.5, 15, 0.25, 0],
  "gpt-5.5": [5, 30, 0.5, 0],
  "gpt-5.6-luna": [0.2, 1.2, 0.02, 0.25],
  "gpt-5.6-sol": [5, 30, 0.5, 6.25],
  "gpt-5.6-terra": [2, 12, 0.2, 2.5],
}

const parameterValues = (parameter) => (parameter.values ?? []).map((value) => value.value)
const isBooleanParameter = (values) =>
  values.length > 0 && values.every((value) => booleanValues.has(value))
const variantKey = (displayName) =>
  displayName.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "") || "variant"
const modelLimit = (modelID, limits, fallback) => {
  let match
  for (const [prefix, limit] of Object.entries(limits)) {
    if (modelID.startsWith(prefix) && (!match || prefix.length > match.prefix.length)) {
      match = { prefix, limit }
    }
  }
  return match?.limit ?? fallback
}
const modelCost = (modelID) => {
  const values = modelLimit(modelID, modelCosts, [0, 0, 0, 0])
  return [{ input: values[0], output: values[1], cache: { read: values[2], write: values[3] } }]
}

export const cursorModelLimits = (modelID, { autoCompaction = false } = {}) => ({
  context: autoCompaction
    ? modelLimit(modelID, contextLimits, 200_000)
    : noAutoCompactionInputLimit,
  ...(autoCompaction ? {} : { input: noAutoCompactionInputLimit }),
  output: modelLimit(modelID, outputLimits, 32_000),
})

export const cursorVariants = (item) => {
  const defaults = {}
  for (const parameter of item.parameters ?? []) {
    if (reasoningParameter.test(parameter.id)) continue
    if (isBooleanParameter(parameterValues(parameter))) defaults[parameter.id] = "false"
  }

  const presets = (item.variants ?? []).filter((variant) => variant.isDefault !== true)
  const presetsShareName =
    presets.length > 1 && new Set(presets.map((variant) => variant.displayName)).size === 1
  const addPlanVariant = (variants, ids) => {
    let id = "plan"
    for (let suffix = 2; ids.has(id); suffix += 1) id = `plan-${suffix}`
    ids.add(id)
    variants.push({ id, headers: {}, body: { mode: "plan", params: defaults } })
  }
  if (presets.length > 0 && !presetsShareName) {
    const variants = []
    const ids = new Set()
    for (const preset of presets) {
      const params = { ...defaults }
      for (const parameter of preset.params ?? []) params[parameter.id] = parameter.value
      const base = variantKey(preset.displayName)
      let id = base
      for (let suffix = 2; ids.has(id); suffix += 1) id = `${base}-${suffix}`
      ids.add(id)
      variants.push({ id, headers: {}, body: { params } })
    }
    addPlanVariant(variants, ids)
    return { defaults, variants }
  }

  const variants = []
  const ids = new Set()
  const hasEffortEnum = (item.parameters ?? []).some((parameter) => {
    const values = parameterValues(parameter)
    return reasoningParameter.test(parameter.id) && !isBooleanParameter(values) && values.length > 0
  })
  const addVariant = (base, params, parameterID) => {
    const id = ids.has(base) ? `${parameterID}-${base}` : base
    ids.add(id)
    variants.push({ id, headers: {}, body: { params } })
  }

  for (const parameter of item.parameters ?? []) {
    const values = parameterValues(parameter)
    if (values.length === 0) continue
    const boolean = isBooleanParameter(values)
    if (reasoningParameter.test(parameter.id)) {
      if (boolean) {
        if (!hasEffortEnum && values.includes("true")) {
          addVariant(parameter.id.toLowerCase(), { ...defaults, [parameter.id]: "true" }, parameter.id)
        }
        continue
      }
      for (const value of values) {
        if (value === "none") continue
        addVariant(
          value === "extra-high" ? "xhigh" : value,
          { ...defaults, [parameter.id]: value },
          parameter.id,
        )
      }
      continue
    }
    if (boolean && values.includes("true")) {
      addVariant(parameter.id.toLowerCase(), { ...defaults, [parameter.id]: "true" }, parameter.id)
    }
  }
  addPlanVariant(variants, ids)
  return { defaults, variants }
}

export const applyCursorCatalog = (catalog, directory, models, bridge) => {
  let autoCompaction = false
  const bridgeHeaders = bridge ? { "x-oy-cursor-bridge": bridge.token } : {}
  catalog.provider.update("cursor", (provider) => {
    provider.name = "Cursor"
    provider.integrationID = "cursor"
    const configured = provider.request?.body ?? {}
    const legacyConfigured = provider.settings ?? {}
    autoCompaction = configured.autoCompaction === true || legacyConfigured.autoCompaction === true
    provider.api = { type: "aisdk", package: cursorProviderPackage }
    provider.package = `aisdk:${cursorProviderPackage}`
    provider.settings = {
      ...legacyConfigured,
      ...configured,
      ...(bridge
        ? {
            baseURL: bridge.url,
            headers: {
              ...(legacyConfigured.headers ?? {}),
              ...(configured.headers ?? {}),
              "x-oy-cursor-bridge": bridge.token,
            },
          }
        : {}),
    }
    provider.request = {
      headers: { ...(provider.request?.headers ?? {}), ...bridgeHeaders },
      body: { ...configured },
    }
    if (bridge) provider.api.url = bridge.url
  })
  for (const item of models) {
    const controls = cursorVariants(item)
    catalog.model.update("cursor", item.id, (model) => {
      model.name = item.displayName || item.id
      model.api = {
        id: item.id,
        type: "aisdk",
        package: cursorProviderPackage,
        ...(bridge ? { url: bridge.url } : {}),
      }
      model.package = `aisdk:${cursorProviderPackage}`
      model.capabilities = { tools: true, input: ["text"], output: ["text"] }
      model.request = {
        headers: { ...bridgeHeaders },
        body: {
          cwd: directory,
          ...(Object.keys(controls.defaults).length > 0 ? { params: controls.defaults } : {}),
        },
      }
      model.settings = {
        ...(Object.keys(controls.defaults).length > 0 ? { params: controls.defaults } : {}),
        ...(Object.keys(bridgeHeaders).length > 0 ? { headers: bridgeHeaders } : {}),
      }
      model.headers = { ...bridgeHeaders }
      model.variants = controls.variants.map((variant) => ({
        ...variant,
        id: variant.id,
        settings: variant.body,
      }))
      model.status = "active"
      model.enabled = true
      model.limit = cursorModelLimits(item.id, { autoCompaction })
      model.cost = modelCost(item.id)
    })
  }
}
