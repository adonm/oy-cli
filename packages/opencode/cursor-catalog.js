export const cursorProviderPackage = "aisdk:@stablekernel/opencode-cursor"

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

export const cursorModelLimits = (modelID, { autoCompaction = false } = {}) => ({
  context: modelLimit(modelID, contextLimits, 200_000),
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
      variants.push({ id, settings: { params } })
    }
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
    variants.push({ id, settings: { params } })
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
  return { defaults, variants }
}

export const applyCursorCatalog = (catalog, directory, models) => {
  let autoCompaction = false
  catalog.provider.update("cursor", (provider) => {
    provider.name = "Cursor"
    provider.integrationID = "cursor"
    provider.package = cursorProviderPackage
    autoCompaction = provider.settings?.autoCompaction === true
    provider.settings = { cwd: directory, ...(provider.settings ?? {}) }
  })
  for (const item of models) {
    const controls = cursorVariants(item)
    catalog.model.update("cursor", item.id, (model) => {
      model.name = item.displayName || item.id
      model.package = cursorProviderPackage
      model.capabilities = { tools: true, input: ["text"], output: ["text"] }
      model.settings = Object.keys(controls.defaults).length > 0 ? { params: controls.defaults } : undefined
      model.variants = controls.variants
      model.status = "active"
      model.enabled = true
      model.limit = cursorModelLimits(item.id, { autoCompaction })
    })
  }
}
