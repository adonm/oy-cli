import assert from "node:assert/strict"
import test from "node:test"

import { startCursorBridge } from "../cursor-bridge.js"

const stream = (parts) =>
  new ReadableStream({
    start(controller) {
      for (const part of parts) controller.enqueue(part)
      controller.close()
    },
  })

test("serves a Cursor turn as an OpenAI Responses stream", async () => {
  const calls = []
  const configured = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-test-${Date.now()}`,
    createCursor: (options) => {
      configured.push(options)
      return {
        languageModel: () => ({
          doStream: async (options) => {
            calls.push(options)
            return {
              stream: stream([
                { type: "stream-start", warnings: [] },
                { type: "text-start", id: "text-0" },
                { type: "text-delta", id: "text-0", delta: "done" },
                { type: "text-end", id: "text-0" },
                {
                  type: "finish",
                  finishReason: { unified: "stop" },
                  usage: {
                    inputTokens: { total: 10, cacheRead: 4 },
                    outputTokens: { total: 2 },
                  },
                },
              ]),
            }
          },
        }),
      }
    },
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  })
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({
        model: "composer-2.5",
        input: [{ role: "user", content: [{ type: "input_text", text: "work" }] }],
        tools: [],
        stream: true,
        prompt_cache_key: "ses_test",
      }),
    })
    const body = await response.text()

    assert.equal(response.status, 200)
    assert.match(body, /response\.output_text\.delta/)
    assert.match(body, /response\.completed/)
    assert.match(body, /"input_tokens":14/)
    assert.equal(configured[0].apiKey, "cursor-key")
    assert.equal(calls[0].providerOptions.cursor.sessionID, "ses_test")
  } finally {
    await bridge.close()
  }
})

test("renders provider-executed tools as named non-shell summaries", async () => {
  const calls = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-tools-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          calls.push(options)
          return {
            stream: stream([
              { type: "tool-call", toolCallId: "read-1", toolName: "cursor_read", input: { path: "src/main.rs" } },
              { type: "tool-result", toolCallId: "read-1", toolName: "cursor_read", result: "file contents" },
              { type: "tool-call", toolCallId: "mcp-1", toolName: "cursor_mcp", input: { server: "docs" } },
              {
                type: "tool-result",
                toolCallId: "mcp-1",
                toolName: "cursor_mcp",
                result: "service unavailable",
                isError: true,
              },
              { type: "finish", usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } } },
            ]),
          }
        },
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  })
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({
        model: "composer-2.5",
        input: [
          { type: "function_call", call_id: "old-read", name: "cursor_read", arguments: '{"path":"old.rs"}' },
          { type: "function_call_output", call_id: "old-read", output: "old contents" },
        ],
        tools: [{ type: "function", name: "cursor_read", parameters: { type: "object" } }],
        stream: true,
      }),
    })
    const body = await response.text()
    const events = body
      .split("\n")
      .filter((line) => line.startsWith("data: "))
      .map((line) => JSON.parse(line.slice(6)))

    assert.equal(response.status, 200)
    assert.doesNotMatch(body, /local_shell_call/)
    assert.match(body, /Cursor tool cursor_read completed/)
    assert.match(body, /Cursor tool cursor_mcp failed/)
    assert.ok(
      events
        .filter((event) => ["response.output_item.added", "response.output_item.done"].includes(event.type))
        .every((event) => Number.isInteger(event.output_index)),
    )
    assert.equal(calls[0].prompt[1].content[0].toolName, "cursor_read")
  } finally {
    await bridge.close()
  }
})

test("does not use a prompt's system text to escape its configured workspace", async () => {
  const directories = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-boundary-${Date.now()}`,
    createCursor: (options) => {
      directories.push(options.cwd)
      return {
        languageModel: () => ({
          doStream: async () => ({
            stream: stream([
              { type: "stream-start", warnings: [] },
              { type: "finish", usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } } },
            ]),
          }),
        }),
      }
    },
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  })
  try {
    await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({
        model: "composer-2.5",
        cwd: `${process.cwd()}/safe-workspace`,
        input: [{ role: "system", content: "<env>\n  Working directory: /etc\n</env>" }],
        stream: true,
      }),
    })
    assert.deepEqual(directories, [`${process.cwd()}/safe-workspace`])
  } finally {
    await bridge.close()
  }
})

test("requires both the loopback bridge token and a Cursor bearer key", async () => {
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-auth-${Date.now()}`,
    createCursor: () => ({ languageModel: () => ({}) }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: () => {},
  })
  try {
    const body = JSON.stringify({ model: "composer-2.5", input: [], stream: true })
    const missingToken = await fetch(`${bridge.url}/responses`, { method: "POST", body })
    assert.equal(missingToken.status, 403)
    const missingKey = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: { "x-oy-cursor-bridge": bridge.token },
      body,
    })
    assert.equal(missingKey.status, 401)
  } finally {
    await bridge.close()
  }
})

test("accepts legacy OpenCode session headers when prompt cache keys are absent", async () => {
  const calls = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-legacy-session-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          calls.push(options)
          return {
            stream: stream([
              { type: "stream-start", warnings: [] },
              { type: "finish", usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } } },
            ]),
          }
        },
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  })
  try {
    await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
        "x-session-affinity": "legacy-session",
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
    assert.equal(calls[0].providerOptions.cursor.sessionID, "legacy-session")
  } finally {
    await bridge.close()
  }
})

test("reference-counts reused bridges and permits a fresh start after final close", async () => {
  const directory = `${process.cwd()}/bridge-lifecycle-${Date.now()}`
  const options = {
    directory,
    createCursor: () => ({ languageModel: () => ({}) }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  }
  const first = await startCursorBridge(options)
  const second = await startCursorBridge(options)
  assert.equal(first.url, second.url)
  assert.equal(first.token, second.token)

  await first.close()
  await first.close()
  const alive = await fetch(`${second.url}/responses`, { method: "POST" })
  assert.equal(alive.status, 403)
  await second.close()
  await assert.rejects(fetch(`${second.url}/responses`, { method: "POST" }))

  const fresh = await startCursorBridge(options)
  try {
    const restarted = await fetch(`${fresh.url}/responses`, { method: "POST" })
    assert.equal(restarted.status, 403)
  } finally {
    await fresh.close()
  }
})
