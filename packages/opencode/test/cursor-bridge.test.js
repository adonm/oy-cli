import assert from "node:assert/strict"
import test from "node:test"
import { createOpenAI } from "@ai-sdk/openai"
import { createOpenAI as createLatestOpenAI } from "@ai-sdk/openai-v4"

import { cursorBridgeLimits, startCursorBridge } from "../cursor-bridge.js"

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
                { type: "text-delta", id: "text-0", delta: "working" },
                { type: "text-end", id: "text-0" },
                { type: "text-start", id: "text-1" },
                { type: "text-delta", id: "text-1", delta: "done" },
                { type: "text-end", id: "text-1" },
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
    assert.match(body, /"input_tokens":10/)
    const textEvents = body
      .split("\n")
      .filter((line) => line.startsWith("data: "))
      .map((line) => JSON.parse(line.slice(6)))
      .filter((event) => event.type.startsWith("response.output_text."))
      .map((event) => [event.type, event.item_id])
    assert.deepEqual(textEvents, [
      ["response.output_text.delta", "text-0"],
      ["response.output_text.done", "text-0"],
      ["response.output_text.delta", "text-1"],
      ["response.output_text.done", "text-1"],
    ])
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
              { type: "stream-start", warnings: [] },
              { type: "tool-call", toolCallId: "read-1", toolName: "cursor_read", input: { path: "src/main.rs" } },
              { type: "tool-call", toolCallId: "mcp-1", toolName: "cursor_mcp", input: { server: "docs" } },
              {
                type: "tool-result",
                toolCallId: "mcp-1",
                toolName: "cursor_mcp",
                result: "service unavailable",
                isError: true,
              },
              { type: "tool-result", toolCallId: "read-1", toolName: "cursor_read", result: "file contents" },
              {
                type: "finish",
                finishReason: { unified: "stop" },
                usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
              },
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
    const completed = events.find((event) => event.type === "response.completed")
    assert.deepEqual(
      completed.response.output.map((item) => item.id),
      ["cursor_tool_read-1", "cursor_tool_mcp-1"],
    )
    assert.equal(calls[0].prompt[1].content[0].toolName, "cursor_read")
  } finally {
    await bridge.close()
  }
})

test("honors explicit and system working directories outside the initial workspace", async () => {
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
              {
                type: "finish",
                finishReason: { unified: "stop" },
                usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
              },
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
  const invoke = (body) =>
    fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", stream: true, ...body }),
    })
  try {
    await (
      await invoke({
        cwd: `${process.cwd()}/safe-workspace`,
        input: [{ role: "system", content: "<env>\n  Working directory: /etc\n</env>" }],
      })
    ).text()
    await (
      await invoke({
        input: [
          {
            role: "system",
            content: "<env>\n  Current session: example\n  Working directory: /outside/worktree\n</env>",
          },
        ],
      })
    ).text()
    assert.deepEqual(directories, [`${process.cwd()}/safe-workspace`, "/outside/worktree"])
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
              {
                type: "finish",
                finishReason: { unified: "stop" },
                usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
              },
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
  const [first, second] = await Promise.all([
    startCursorBridge(options),
    startCursorBridge(options),
  ])
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

test("round-trips through OpenCode's and the latest OpenAI Responses parsers", async () => {
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-parser-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async () => ({
          stream: stream([
            { type: "stream-start", warnings: [] },
            { type: "reasoning-start", id: "reasoning-0" },
            { type: "reasoning-delta", id: "reasoning-0", delta: "checking" },
            { type: "reasoning-end", id: "reasoning-0" },
            { type: "text-start", id: "text-0" },
            { type: "text-delta", id: "text-0", delta: "done" },
            { type: "text-end", id: "text-0" },
            {
              type: "finish",
              finishReason: { unified: "stop" },
              usage: {
                inputTokens: { total: 15, noCache: 10, cacheRead: 4, cacheWrite: 1 },
                outputTokens: { total: 2, reasoning: 1 },
              },
            },
          ]),
        }),
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
  })
  try {
    for (const factory of [createOpenAI, createLatestOpenAI]) {
      const openai = factory({
        apiKey: "cursor-key",
        baseURL: bridge.url,
        headers: { "x-oy-cursor-bridge": bridge.token },
      })
      const result = await openai.responses("composer-2.5").doStream({
        prompt: [{ role: "user", content: [{ type: "text", text: "work" }] }],
      })
      const parts = []
      for await (const part of result.stream) parts.push(part)

      assert.deepEqual(
        parts
          .filter((part) => ["text-start", "text-delta", "text-end"].includes(part.type))
          .map((part) => [part.type, part.id]),
        [
          ["text-start", "text-0"],
          ["text-delta", "text-0"],
          ["text-end", "text-0"],
        ],
      )
      const finish = parts.find((part) => part.type === "finish")
      assert.equal(finish.finishReason.unified, "stop")
      assert.equal(finish.usage.inputTokens.total, 15)
      assert.equal(finish.usage.inputTokens.cacheRead, 4)
      assert.equal(finish.usage.inputTokens.cacheWrite, 1)
      assert.equal(finish.usage.outputTokens.total, 2)
      assert.equal(finish.usage.outputTokens.reasoning, 1)
    }
  } finally {
    await bridge.close()
  }
})

test("rejects malformed provider streams instead of completing them", async () => {
  const errors = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-malformed-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async () => ({
          stream: stream([
            { type: "stream-start", warnings: [] },
            { type: "text-start", id: "text-0" },
            { type: "text-delta", id: "text-0", delta: "unterminated" },
            {
              type: "finish",
              finishReason: { unified: "stop" },
              usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
            },
          ]),
        }),
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: (operation, error) => errors.push([operation, error]),
  })
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
    const body = await response.text()

    assert.match(body, /cursor_stream_protocol_error/)
    assert.doesNotMatch(body, /response\.completed/)
    assert.equal(errors.length, 1)
    assert.equal(errors[0][1].name, "CursorBridgeProtocolError")
  } finally {
    await bridge.close()
  }
})

test("uses explicit side-call metadata instead of guessing from tools", async () => {
  const calls = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-ephemeral-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          calls.push(options)
          return {
            stream: stream([
              { type: "stream-start", warnings: [] },
              {
                type: "finish",
                finishReason: { unified: "stop" },
                usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
              },
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
  const invoke = (metadata) =>
    fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({
        model: "composer-2.5",
        input: [],
        tools: [],
        metadata,
        stream: true,
      }),
    })
  try {
    await (await invoke()).text()
    await (await invoke({ oy_ephemeral: "true" })).text()

    assert.equal(calls[0].providerOptions.cursor.ephemeral, undefined)
    assert.equal(calls[1].providerOptions.cursor.ephemeral, true)
  } finally {
    await bridge.close()
  }
})

test("bounds retained tool history and truncates oversized values", async () => {
  const calls = []
  const manyTools = [{ type: "stream-start", warnings: [] }]
  for (let index = 0; index <= cursorBridgeLimits.historyEntries; index += 1) {
    manyTools.push(
      { type: "tool-call", toolCallId: `call-${index}`, toolName: "read", input: { index } },
      {
        type: "tool-result",
        toolCallId: `call-${index}`,
        toolName: "read",
        result:
          index === cursorBridgeLimits.historyEntries
            ? `${"x".repeat(cursorBridgeLimits.historyValueBytes - 1)}💥${"x".repeat(100_000)}`
            : `result-${index}`,
      },
    )
  }
  manyTools.push({
    type: "finish",
    finishReason: { unified: "stop" },
    usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
  })
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-history-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          calls.push(options)
          return {
            stream:
              calls.length === 1
                ? stream(manyTools)
                : stream([
                    { type: "stream-start", warnings: [] },
                    {
                      type: "finish",
                      finishReason: { unified: "stop" },
                      usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
                    },
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
  const invoke = (input) =>
    fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({
        model: "composer-2.5",
        prompt_cache_key: "history-session",
        input,
        stream: true,
      }),
    })
  try {
    await (await invoke([])).text()
    await (
      await invoke([
        { type: "item_reference", id: "call-0" },
        {
          type: "item_reference",
          id: `cursor_tool_call-${cursorBridgeLimits.historyEntries}`,
        },
      ])
    ).text()

    assert.equal(calls[1].prompt.length, 2)
    const retained = calls[1].prompt[1].content
    assert.equal(retained[0].toolCallId, `cursor_tool_call-${cursorBridgeLimits.historyEntries}`)
    assert.equal(retained[1].output.value.truncated, true)
    assert.ok(Buffer.byteLength(retained[1].output.value.preview) < 70_000)
    assert.doesNotMatch(retained[1].output.value.preview, /\uFFFD/)
  } finally {
    await bridge.close()
  }
})

test("retries a failed model refresh after a bounded cooldown", async () => {
  let attempts = 0
  const discovered = []
  const failures = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-refresh-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async () => ({
          stream: stream([
            { type: "stream-start", warnings: [] },
            {
              type: "finish",
              finishReason: { unified: "stop" },
              usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
            },
          ]),
        }),
      }),
    }),
    listCursorModels: async () => {
      attempts += 1
      if (attempts === 1) throw new Error("temporary failure")
      return [{ id: "live" }]
    },
    onModels: async (models) => discovered.push(models),
    onDirectory: () => {},
    onIdle: () => {},
    reportError: (operation) => failures.push(operation),
    modelRefreshRetryMs: 1,
  })
  const invoke = () =>
    fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
  try {
    await (await invoke()).text()
    await new Promise((resolve) => setTimeout(resolve, 5))
    await (await invoke()).text()
    for (let count = 0; count < 20 && discovered.length === 0; count += 1) {
      await new Promise((resolve) => setImmediate(resolve))
    }

    assert.equal(attempts, 2)
    assert.equal(failures.length, 1)
    assert.deepEqual(discovered, [[{ id: "live" }]])
  } finally {
    await bridge.close()
  }
})

test("bounds model discovery without blocking a model turn", async () => {
  const failures = []
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-refresh-timeout-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async () => ({
          stream: stream([
            { type: "stream-start", warnings: [] },
            {
              type: "finish",
              finishReason: { unified: "stop" },
              usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
            },
          ]),
        }),
      }),
    }),
    listCursorModels: () => new Promise(() => {}),
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: (operation, error) => failures.push([operation, error]),
    modelRefreshTimeoutMs: 5,
  })
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
    assert.match(await response.text(), /response\.completed/)
    for (let count = 0; count < 20 && failures.length === 0; count += 1) {
      await new Promise((resolve) => setTimeout(resolve, 1))
    }
    assert.equal(failures.length, 1)
    assert.match(failures[0][1].message, /exceeded 5 ms/)
  } finally {
    await bridge.close()
  }
})

test("emits SSE heartbeats while Cursor is quiet", async () => {
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-heartbeat-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async () => ({
          stream: new ReadableStream({
            async start(controller) {
              controller.enqueue({ type: "stream-start", warnings: [] })
              await new Promise((resolve) => setTimeout(resolve, 20))
              controller.enqueue({
                type: "finish",
                finishReason: { unified: "stop" },
                usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
              })
              controller.close()
            },
          }),
        }),
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: assert.fail,
    heartbeatIntervalMs: 5,
  })
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
    assert.match(await response.text(), /: heartbeat/)
  } finally {
    await bridge.close()
  }
})

test("aborts active Cursor runs during final bridge shutdown", async () => {
  let providerSignal
  let resolveAborted
  const aborted = new Promise((resolve) => {
    resolveAborted = resolve
  })
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-abort-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          providerSignal = options.abortSignal
          return {
            stream: new ReadableStream({
              start(controller) {
                controller.enqueue({ type: "stream-start", warnings: [] })
                options.abortSignal.addEventListener(
                  "abort",
                  () => {
                    resolveAborted()
                    const error = new Error("aborted")
                    error.name = "AbortError"
                    controller.error(error)
                  },
                  { once: true },
                )
              },
            }),
          }
        },
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: () => {},
  })
  const response = await fetch(`${bridge.url}/responses`, {
    method: "POST",
    headers: {
      authorization: "Bearer cursor-key",
      "content-type": "application/json",
      "x-oy-cursor-bridge": bridge.token,
    },
    body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
  })

  assert.equal(response.status, 200)
  await bridge.close()
  await aborted
  assert.equal(providerSignal.aborted, true)
  await response.text().catch(() => "")
})

test("aborts a Cursor run when the OpenCode client disconnects", async () => {
  let calls = 0
  let resolveAborted
  const aborted = new Promise((resolve) => {
    resolveAborted = resolve
  })
  const bridge = await startCursorBridge({
    directory: `${process.cwd()}/bridge-disconnect-${Date.now()}`,
    createCursor: () => ({
      languageModel: () => ({
        doStream: async (options) => {
          calls += 1
          if (calls > 1) {
            return {
              stream: stream([
                { type: "stream-start", warnings: [] },
                {
                  type: "finish",
                  finishReason: { unified: "stop" },
                  usage: { inputTokens: { total: 1 }, outputTokens: { total: 1 } },
                },
              ]),
            }
          }
          return {
            stream: new ReadableStream({
              start(controller) {
                controller.enqueue({ type: "stream-start", warnings: [] })
                options.abortSignal.addEventListener(
                  "abort",
                  () => {
                    resolveAborted()
                    const error = new Error("aborted")
                    error.name = "AbortError"
                    controller.error(error)
                  },
                  { once: true },
                )
              },
            }),
          }
        },
      }),
    }),
    listCursorModels: async () => [],
    onModels: async () => {},
    onDirectory: () => {},
    onIdle: () => {},
    reportError: () => {},
  })
  const controller = new AbortController()
  try {
    const response = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
      signal: controller.signal,
    })
    controller.abort()
    await response.text().catch(() => "")
    await aborted

    const healthy = await fetch(`${bridge.url}/responses`, {
      method: "POST",
      headers: {
        authorization: "Bearer cursor-key",
        "content-type": "application/json",
        "x-oy-cursor-bridge": bridge.token,
      },
      body: JSON.stringify({ model: "composer-2.5", input: [], stream: true }),
    })
    assert.match(await healthy.text(), /response\.completed/)
  } finally {
    await bridge.close()
  }
})
