import { createHash, randomBytes } from "node:crypto"
import { createServer } from "node:http"
import { isAbsolute, relative, resolve } from "node:path"

import { createQualityCursor } from "./cursor-provider.js"

const maxRequestBytes = 16 * 1024 * 1024
const bridges = new Map()

const text = (content) => {
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return ""
  return content
    .flatMap((part) => {
      if (part?.type === "input_text" || part?.type === "output_text") return [part.text ?? ""]
      if (part?.type === "input_image") return [`[attached image: ${part.image_url ?? "image"} — not forwarded to Cursor]`]
      return []
    })
    .join("\n")
}

const outputValue = (value) => {
  if (typeof value === "string") return { type: "text", value }
  return { type: "json", value: value ?? null }
}

const promptFromResponses = (body, toolHistory) => {
  const calls = new Map(
    (body.input ?? [])
      .filter((item) => item.type === "function_call")
      .map((item) => [item.call_id, item.name]),
  )
  return (body.input ?? []).flatMap((item) => {
    if (item.role === "system") return [{ role: "system", content: text(item.content) }]
    if (item.role === "user") {
      return [{ role: "user", content: [{ type: "text", text: text(item.content) }] }]
    }
    if (item.role === "assistant") {
      return [{ role: "assistant", content: [{ type: "text", text: text(item.content) }] }]
    }
    if (item.type === "reasoning") {
      const summary = (item.summary ?? []).map((part) => part.text ?? "").join("\n")
      return summary ? [{ role: "assistant", content: [{ type: "reasoning", text: summary }] }] : []
    }
    if (item.type === "function_call") {
      let input = item.arguments ?? "{}"
      try {
        input = JSON.parse(input)
      } catch {
        // Preserve malformed historical input as text for Cursor's transcript.
      }
      return [
        {
          role: "assistant",
          content: [{ type: "tool-call", toolCallId: item.call_id, toolName: item.name, input }],
        },
      ]
    }
    if (item.type === "function_call_output") {
      return [
        {
          role: "tool",
          content: [
            {
              type: "tool-result",
              toolCallId: item.call_id,
              toolName: calls.get(item.call_id) ?? toolHistory.get(item.call_id)?.name ?? "unknown_tool",
              output: outputValue(item.output),
            },
          ],
        },
      ]
    }
    if (item.type === "item_reference") {
      const previous = toolHistory.get(item.id)
      if (!previous) return []
      return [
        {
          role: "assistant",
          content: [
            {
              type: "tool-call",
              toolCallId: item.id,
              toolName: previous.name,
              input: previous.input,
            },
            {
              type: "tool-result",
              toolCallId: item.id,
              toolName: previous.name,
              output: outputValue(previous.result),
            },
          ],
        },
      ]
    }
    return []
  })
}

const requestDirectory = (body, fallback) => {
  if (typeof body.cwd === "string" && isAbsolute(body.cwd)) return resolve(body.cwd)
  const system = (body.input ?? []).find((item) => item.role === "system")
  const match = text(system?.content).match(/<env>\s*Working directory:\s*([^\r\n<]+)/)
  const candidate = match?.[1]?.trim()
  if (!candidate || !isAbsolute(candidate)) return fallback
  const root = resolve(fallback)
  const path = resolve(candidate)
  const outside = relative(root, path).startsWith("..")
  return outside ? fallback : path
}

const toolsFromResponses = (tools) =>
  (tools ?? []).flatMap((tool) =>
    tool.type === "function"
      ? [
          {
            type: "function",
            name: tool.name,
            description: tool.description,
            inputSchema: tool.parameters ?? { type: "object", properties: {} },
          },
        ]
      : [],
  )

const toolChoiceFromResponses = (choice) => {
  if (choice === "none" || choice === "required" || choice === "auto") return { type: choice }
  if (choice?.type === "function") return { type: "tool", toolName: choice.name }
}

const providerOptions = (body, apiKey, directory) => ({
  apiKey,
  cwd: directory,
  ...(body.mode === "plan" ? { mode: "plan" } : {}),
  ...(body.params && typeof body.params === "object" ? { params: body.params } : {}),
  ...(Array.isArray(body.settingSources) ? { settingSources: body.settingSources } : {}),
  ...(typeof body.sandbox === "boolean" ? { sandbox: body.sandbox } : {}),
  ...(typeof body.autoReview === "boolean" ? { autoReview: body.autoReview } : {}),
  ...(body.agents && typeof body.agents === "object" ? { agents: body.agents } : {}),
  ...(body.mcpServers && typeof body.mcpServers === "object" ? { mcpServers: body.mcpServers } : {}),
  ...(body.session === false ? { session: false } : { session: "auto" }),
  ...(body.toolDisplay === "reasoning" ? { toolDisplay: "reasoning" } : {}),
  ...(["rules", "message", "omit"].includes(body.systemPrompt)
    ? { systemPrompt: body.systemPrompt }
    : {}),
  ...(["http1", "http2-direct", "sidecar"].includes(body.transport)
    ? { transport: body.transport }
    : {}),
})

const usageFromFinish = (usage) => ({
  input_tokens:
    (usage?.inputTokens?.total ?? 0) +
    (usage?.inputTokens?.cacheRead ?? 0) +
    (usage?.inputTokens?.cacheWrite ?? 0),
  input_tokens_details: { cached_tokens: usage?.inputTokens?.cacheRead ?? 0 },
  output_tokens: usage?.outputTokens?.total ?? 0,
  output_tokens_details: { reasoning_tokens: usage?.outputTokens?.reasoning ?? 0 },
  total_tokens:
    (usage?.inputTokens?.total ?? 0) +
    (usage?.inputTokens?.cacheRead ?? 0) +
    (usage?.inputTokens?.cacheWrite ?? 0) +
    (usage?.outputTokens?.total ?? 0),
})

const readBody = (request) =>
  new Promise((resolve, reject) => {
    const chunks = []
    let size = 0
    request.on("data", (chunk) => {
      size += chunk.length
      if (size > maxRequestBytes) {
        reject(new Error("request body exceeds 16 MiB"))
        request.destroy()
        return
      }
      chunks.push(chunk)
    })
    request.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")))
      } catch (error) {
        reject(error)
      }
    })
    request.on("error", reject)
  })

const bearer = (request) => {
  const value = request.headers.authorization
  return typeof value === "string" && value.startsWith("Bearer ") ? value.slice(7) : undefined
}

const requestSession = (request) =>
  ["x-session-id", "x-session-affinity", "x-opencode-session"]
    .map((name) => request.headers[name])
    .find((value) => typeof value === "string" && value.length > 0)

const sendEvent = (response, event) => response.write(`data: ${JSON.stringify(event)}\n\n`)

const displayValue = (value) => {
  let rendered
  if (typeof value === "string") rendered = value
  else {
    try {
      rendered = JSON.stringify(value ?? null)
    } catch {
      rendered = String(value)
    }
  }
  const limit = 16 * 1024
  return rendered.length <= limit ? rendered : `${rendered.slice(0, limit)}\n… [tool output truncated]`
}

const sendToolSummary = (response, call, part, outputIndex) => {
  const id = `cursor_tool_${part.toolCallId}`
  const status = part.isError ? "failed" : "completed"
  const summary = [
    `Cursor tool ${call.name} ${status}`,
    `Input: ${displayValue(call.input)}`,
    `${part.isError ? "Error" : "Result"}: ${displayValue(part.result)}`,
  ].join("\n")
  sendEvent(response, {
    type: "response.output_item.added",
    output_index: outputIndex,
    item: { type: "reasoning", id, summary: [], encrypted_content: null },
  })
  sendEvent(response, { type: "response.reasoning_summary_part.added", item_id: id, summary_index: 0 })
  sendEvent(response, {
    type: "response.reasoning_summary_text.delta",
    item_id: id,
    summary_index: 0,
    delta: summary,
  })
  sendEvent(response, { type: "response.reasoning_summary_part.done", item_id: id, summary_index: 0 })
  sendEvent(response, {
    type: "response.output_item.done",
    output_index: outputIndex,
    item: {
      type: "reasoning",
      id,
      summary: [{ type: "summary_text", text: summary }],
      encrypted_content: null,
    },
  })
}

const streamCursor = async ({
  body,
  apiKey,
  directory,
  createCursor,
  response,
  toolHistory,
  abortSignal,
  requestSessionID,
}) => {
  const sdk = createQualityCursor(createCursor, providerOptions(body, apiKey, directory))
  const language = sdk.languageModel(body.model)
  const sessionID = body.prompt_cache_key ?? requestSessionID
  const options = {
    prompt: promptFromResponses(body, toolHistory),
    tools: toolsFromResponses(body.tools),
    toolChoice: toolChoiceFromResponses(body.tool_choice),
    maxOutputTokens: body.max_output_tokens,
    temperature: body.temperature,
    topP: body.top_p,
    providerOptions: { cursor: { ...(sessionID ? { sessionID } : {}) } },
    abortSignal,
  }
  if (body.tool_choice === "none" || !Array.isArray(body.tools) || body.tools.length === 0) {
    options.prompt.unshift({
      role: "system",
      content: "This is a text-only side call. Do not use tools, modify files, or run commands.",
    })
    options.providerOptions.cursor.ephemeral = true
  }

  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
  })
  const result = await language.doStream(options)
  const openTools = new Map()
  const reasoning = new Map()
  let nextOutputIndex = 0
  let responseID = `resp_${randomBytes(12).toString("hex")}`
  let finishUsage
  for await (const part of result.stream) {
    if (part.type === "text-delta") {
      sendEvent(response, { type: "response.output_text.delta", item_id: part.id, delta: part.delta })
      continue
    }
    if (part.type === "text-end") {
      sendEvent(response, { type: "response.output_text.done", item_id: part.id })
      continue
    }
    if (part.type === "reasoning-start") {
      const outputIndex = nextOutputIndex
      nextOutputIndex += 1
      reasoning.set(part.id, outputIndex)
      sendEvent(response, {
        type: "response.output_item.added",
        output_index: outputIndex,
        item: { type: "reasoning", id: part.id, summary: [], encrypted_content: null },
      })
      sendEvent(response, { type: "response.reasoning_summary_part.added", item_id: part.id, summary_index: 0 })
      continue
    }
    if (part.type === "reasoning-delta") {
      sendEvent(response, {
        type: "response.reasoning_summary_text.delta",
        item_id: part.id,
        summary_index: 0,
        delta: part.delta,
      })
      continue
    }
    if (part.type === "reasoning-end") {
      const outputIndex = reasoning.get(part.id) ?? nextOutputIndex++
      sendEvent(response, { type: "response.reasoning_summary_part.done", item_id: part.id, summary_index: 0 })
      sendEvent(response, {
        type: "response.output_item.done",
        output_index: outputIndex,
        item: { type: "reasoning", id: part.id, summary: [], encrypted_content: null },
      })
      reasoning.delete(part.id)
      continue
    }
    if (part.type === "tool-call") {
      openTools.set(part.toolCallId, { name: part.toolName, input: part.input })
      continue
    }
    if (part.type === "tool-result") {
      const call = openTools.get(part.toolCallId) ?? { name: part.toolName, input: {} }
      openTools.delete(part.toolCallId)
      toolHistory.set(part.toolCallId, { name: call.name, input: call.input, result: part.result })
      if (toolHistory.size > 2_000) toolHistory.delete(toolHistory.keys().next().value)
      sendToolSummary(response, call, part, nextOutputIndex)
      nextOutputIndex += 1
      continue
    }
    if (part.type === "error") throw part.error
    if (part.type === "finish") {
      finishUsage = usageFromFinish(part.usage)
      responseID = part.providerMetadata?.cursor?.responseId ?? responseID
    }
  }
  for (const [id, outputIndex] of reasoning) {
    sendEvent(response, { type: "response.reasoning_summary_part.done", item_id: id, summary_index: 0 })
    sendEvent(response, {
      type: "response.output_item.done",
      output_index: outputIndex,
      item: { type: "reasoning", id, summary: [], encrypted_content: null },
    })
  }
  sendEvent(response, {
    type: "response.completed",
    response: { id: responseID, usage: finishUsage ?? usageFromFinish() },
  })
  response.end()
}

export const startCursorBridge = async ({
  directory,
  createCursor,
  listCursorModels,
  onModels,
  onDirectory,
  onIdle,
  reportError,
}) => {
  const handlers = { createCursor, listCursorModels, onModels, onDirectory, onIdle, reportError }
  const existing = bridges.get(directory)
  if (existing) {
    return createBridgeLease(existing, handlers)
  }

  const token = randomBytes(32).toString("base64url")
  const refreshedKeys = new Set()
  const toolHistories = new Map()
  const active = new Map()
  const state = {
    current: handlers,
    owners: [],
    closing: undefined,
  }
  const server = createServer(async (request, response) => {
    if (request.method !== "POST" || !request.url?.endsWith("/responses")) {
      response.writeHead(404).end()
      return
    }
    if (request.headers["x-oy-cursor-bridge"] !== token) {
      response.writeHead(403).end()
      return
    }
    const apiKey = bearer(request)
    if (!apiKey) {
      response.writeHead(401).end()
      return
    }
    try {
      const body = await readBody(request)
      const cwd = requestDirectory(body, directory)
      state.current.onDirectory(cwd)
      active.set(cwd, (active.get(cwd) ?? 0) + 1)
      const fingerprint = createHash("sha256").update(apiKey).digest("hex")
      const session = body.prompt_cache_key ?? requestSession(request) ?? "ephemeral"
      const historyKey = `${fingerprint}:${cwd}:${session}`
      const toolHistory = toolHistories.get(historyKey) ?? new Map()
      toolHistories.set(historyKey, toolHistory)
      if (toolHistories.size > 128) toolHistories.delete(toolHistories.keys().next().value)
      if (!refreshedKeys.has(fingerprint)) {
        refreshedKeys.add(fingerprint)
        void state.current
          .listCursorModels(apiKey)
          .then(state.current.onModels)
          .catch((error) => state.current.reportError("failed to refresh Cursor models from bridge", error))
      }
      try {
        const abort = new AbortController()
        const abortRequest = () => abort.abort()
        request.once("aborted", abortRequest)
        response.once("close", abortRequest)
        try {
          await streamCursor({
            body,
            apiKey,
            directory: cwd,
            createCursor: state.current.createCursor,
            response,
            toolHistory,
            abortSignal: abort.signal,
            requestSessionID: requestSession(request),
          })
        } finally {
          request.off("aborted", abortRequest)
          response.off("close", abortRequest)
        }
      } finally {
        const remaining = (active.get(cwd) ?? 1) - 1
        if (remaining > 0) active.set(cwd, remaining)
        else {
          active.delete(cwd)
          state.current.onIdle(cwd)
        }
      }
    } catch (error) {
      state.current.reportError("Cursor bridge request failed", error)
      if (response.writableEnded) return
      if (!response.headersSent) {
        response.writeHead(400, { "content-type": "application/json" })
        response.end(JSON.stringify({ error: "Cursor bridge request failed" }))
        return
      }
      sendEvent(response, {
        type: "error",
        code: "cursor_bridge_error",
        message: error instanceof Error ? error.message : String(error),
      })
      response.end()
    }
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  server.unref()
  const address = server.address()
  if (!address || typeof address === "string") throw new Error("Cursor bridge did not bind a TCP port")
  Object.assign(state, {
    directory,
    url: `http://127.0.0.1:${address.port}/v1`,
    token,
    server,
    clear: () => {
      refreshedKeys.clear()
      toolHistories.clear()
    },
  })
  bridges.set(directory, state)
  return createBridgeLease(state, handlers)
}

const createBridgeLease = (state, handlers) => {
  const owner = { handlers }
  state.owners.push(owner)
  state.current = handlers
  let released = false
  return {
    url: state.url,
    token: state.token,
    close: async () => {
      if (released) return state.closing
      released = true
      const index = state.owners.indexOf(owner)
      if (index !== -1) state.owners.splice(index, 1)
      if (state.owners.length > 0) {
        state.current = state.owners.at(-1).handlers
        return
      }
      if (bridges.get(state.directory) === state) bridges.delete(state.directory)
      state.clear()
      state.closing ??= new Promise((resolve, reject) => {
        state.server.close((error) => {
          if (error) reject(error)
          else resolve()
        })
      })
      return state.closing
    },
  }
}
