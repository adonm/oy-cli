import assert from "node:assert/strict"
import test from "node:test"

import { applyCursorCatalog, cursorModelLimits, cursorVariants } from "../cursor-catalog.js"

test("uses Cursor model-specific limits and suppresses host auto-compaction", () => {
  assert.deepEqual(cursorModelLimits("gpt-5.6-sol"), {
    context: 272_000,
    input: 1_000_000_000,
    output: 64_000,
  })
  assert.deepEqual(cursorModelLimits("claude-opus-4-8-thinking"), {
    context: 300_000,
    input: 1_000_000_000,
    output: 64_000,
  })
  assert.deepEqual(cursorModelLimits("future-model"), {
    context: 200_000,
    input: 1_000_000_000,
    output: 32_000,
  })
})

test("leaves the input threshold unset when auto-compaction is enabled", () => {
  assert.deepEqual(cursorModelLimits("gpt-5.6-sol", { autoCompaction: true }), {
    context: 272_000,
    output: 64_000,
  })
})

test("applies the provider auto-compaction setting to catalog models", () => {
  let configuredModel
  const catalog = {
    provider: {
      update(_id, apply) {
        apply({ settings: { autoCompaction: true } })
      },
    },
    model: {
      update(_provider, _model, apply) {
        configuredModel = {}
        apply(configuredModel)
      },
    },
  }

  applyCursorCatalog(catalog, "/workspace", [{ id: "gpt-5.6-sol", displayName: "GPT-5.6 Sol" }])

  assert.deepEqual(configuredModel.limit, { context: 272_000, output: 64_000 })
})

test("builds off and on variants for Cursor thinking controls", () => {
  const controls = cursorVariants({
    parameters: [{ id: "thinking", values: [{ value: "off" }, { value: "on" }] }],
  })

  assert.deepEqual(controls.defaults, {})
  assert.deepEqual(
    controls.variants.map((variant) => variant.id),
    ["off", "on"],
  )
  assert.deepEqual(controls.variants[0].settings.params, { thinking: "off" })
})

test("builds defaults and reasoning variants using Cursor semantics", () => {
  const controls = cursorVariants({
    parameters: [
      { id: "fast", values: [{ value: "false" }, { value: "true" }] },
      {
        id: "reasoning_effort",
        values: [
          { value: "none" },
          { value: "low" },
          { value: "off" },
          { value: "extra-high" },
        ],
      },
    ],
  })

  assert.deepEqual(controls.defaults, { fast: "false" })
  assert.deepEqual(
    controls.variants.map((variant) => variant.id),
    ["fast", "low", "off", "xhigh"],
  )
  assert.deepEqual(controls.variants.at(-1).settings.params, {
    fast: "false",
    reasoning_effort: "extra-high",
  })
})

test("prefers labeled Cursor SDK presets", () => {
  const controls = cursorVariants({
    parameters: [{ id: "fast", values: [{ value: "false" }, { value: "true" }] }],
    variants: [
      { displayName: "Default", isDefault: true, params: [] },
      {
        displayName: "High Quality",
        params: [{ id: "thinking", value: "on" }],
      },
      {
        displayName: "High Quality",
        params: [{ id: "thinking", value: "off" }],
      },
      {
        displayName: "Fast Mode",
        params: [{ id: "fast", value: "true" }],
      },
    ],
  })

  assert.deepEqual(
    controls.variants.map((variant) => variant.id),
    ["high-quality", "high-quality-2", "fast-mode"],
  )
  assert.deepEqual(controls.variants[0].settings.params, {
    fast: "false",
    thinking: "on",
  })
})
