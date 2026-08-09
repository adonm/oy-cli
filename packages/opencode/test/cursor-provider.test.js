import assert from "node:assert/strict"
import test from "node:test"

import { createQualityCursor } from "../cursor-provider.js"

const createFake = (calls, configured) => (options) => {
  configured.push(options)
  return {
    specificationVersion: "v3",
    languageModel: () => ({
      provider: "cursor",
      modelId: "model",
      specificationVersion: "v3",
      doGenerate: async (options) => {
        calls.push(options)
        return options
      },
      doStream: async (options) => {
        calls.push(options)
        return options
      },
    }),
  }
}

test("injects the OpenCode session id while preserving request controls", async () => {
  const calls = []
  const configured = []
  const provider = createQualityCursor(createFake(calls, configured), {
    name: "cursor",
    agents: { custom: { description: "Custom", prompt: "Custom", model: "inherit" } },
  })

  await provider.languageModel("model").doStream({
    headers: { "x-session-affinity": "ses_123" },
    prompt: [{ role: "user", content: [{ type: "text", text: "continue" }] }],
    tools: [{ type: "function", name: "read" }],
    providerOptions: { cursor: { params: { thinking: "high" } } },
  })

  assert.equal(configured[0].session, "auto")
  assert.deepEqual(configured[0].settingSources, ["project"])
  assert.equal(configured[0].agents.custom.description, "Custom")
  assert.equal(calls[0].providerOptions.cursor.sessionID, "ses_123")
  assert.equal(calls[0].providerOptions.cursor.ephemeral, undefined)
  assert.deepEqual(calls[0].providerOptions.cursor.params, { thinking: "high" })
})

test("does not infer ephemeral side calls from an absent tool list", async () => {
  const calls = []
  const provider = createQualityCursor(createFake(calls, []), {})

  await provider.languageModel("model").doGenerate({
    headers: { "X-Session-Id": "ses_title" },
    prompt: [{ role: "user", content: [{ type: "text", text: "title" }] }],
  })

  assert.equal(calls[0].providerOptions.cursor.sessionID, "ses_title")
  assert.equal(calls[0].providerOptions.cursor.ephemeral, undefined)
})

test("preserves an explicit ephemeral side-call marker", async () => {
  const calls = []
  const provider = createQualityCursor(createFake(calls, []), {})

  await provider.languageModel("model").doGenerate({
    prompt: [{ role: "user", content: [{ type: "text", text: "title" }] }],
    providerOptions: { cursor: { ephemeral: true } },
  })

  assert.equal(calls[0].providerOptions.cursor.ephemeral, true)
})
