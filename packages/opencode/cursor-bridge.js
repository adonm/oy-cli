import { createHash, randomBytes, timingSafeEqual } from "node:crypto"
import { createServer } from "node:http"
import { isAbsolute, resolve } from "node:path"

import { createQualityCursor } from "./cursor-provider.js"

const maxRequestBytes = 16 * 1024 * 1024
export const cursorBridgeLimits = Object.freeze({
  heartbeatMs: 15_000,
  historyEntries: 256,
  historyBytes: 2 * 1024 * 1024,
  historyValueBytes: 64 * 1024,
  histories: 64,
  historyTtlMs: 60 * 60 * 1_000,
  modelRefreshMs: 15_000,
  modelRefreshRetryMs: 30_000,
  shutdownForceMs: 250,
  shutdownMs: 5_000,
})
const bridges = new Map()

class CursorBridgeProtocolError extends Error {
  constructor(message) {
    super(`invalid Cursor stream: ${message}`)
    this.name = "CursorBridgeProtocolError"
    this.code = "cursor_stream_protocol_error"
  }
}

const withTimeout = async (promise, milliseconds, operation) => {
  let timeout
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timeout = setTimeout(() => {
          reject(new Error(`${operation} exceeded ${milliseconds} ms`))
        }, milliseconds)
        timeout.unref?.()
      }),
    ])
  } finally {
    clearTimeout(timeout)
  }
}

const truncateUtf8 = (value, bytes) => {
  const encoded = Buffer.from(value)
  if (encoded.length <= bytes) return value
  let end = bytes
  while (end > 0 && (encoded[end] & 0xc0) === 0x80) end -= 1
  return `${encoded.subarray(0, end).toString("utf8")}\n… [history value truncated]`
}

const boundedHistoryValue = (value) => {
  let serialized
  try {
    serialized = typeof value === "string" ? value : JSON.stringify(value ?? null)
  } catch {
    serialized = String(value)
  }
  if (Buffer.byteLength(serialized) <= cursorBridgeLimits.historyValueBytes) return value ?? null
  return {
    truncated: true,
    preview: truncateUtf8(serialized, cursorBridgeLimits.historyValueBytes),
  }
}

const entryBytes = (entry) => {
  try {
    return Buffer.byteLength(JSON.stringify(entry))
  } catch {
    return cursorBridgeLimits.historyValueBytes
  }
}

const createToolHistory = () => ({ items: new Map(), bytes: 0, touchedAt: Date.now() })

const rememberTool = (history, id, call, result) => {
  const entry = {
    name: call.name,
    input: boundedHistoryValue(call.input),
    result: boundedHistoryValue(result),
  }
  const previous = history.items.get(id)
  if (previous) history.bytes -= entryBytes(previous)
  history.items.delete(id)
  history.items.set(id, entry)
  history.bytes += entryBytes(entry)
  history.touchedAt = Date.now()
  while (
    history.items.size > cursorBridgeLimits.historyEntries ||
    history.bytes > cursorBridgeLimits.historyBytes
  ) {
    const oldest = history.items.keys().next().value
    if (oldest === undefined) break
    const removed = history.items.get(oldest)
    history.items.delete(oldest)
    history.bytes -= entryBytes(removed)
  }
}

const historyFor = (histories, key) => {
  const now = Date.now()
  for (const [candidate, history] of histories) {
    if (now - history.touchedAt > cursorBridgeLimits.historyTtlMs) histories.delete(candidate)
  }
  let history = histories.get(key)
  if (history) histories.delete(key)
  else history = createToolHistory()
  history.touchedAt = now
  histories.set(key, history)
  while (histories.size > cursorBridgeLimits.histories) {
    histories.delete(histories.keys().next().value)
  }
  return history
}

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
      const referencedToolID = item.id.startsWith("cursor_tool_result_")
        ? item.id.slice("cursor_tool_result_".length)
        : item.id.startsWith("cursor_tool_")
          ? item.id.slice("cursor_tool_".length)
          : item.id
      const previous =
        toolHistory.get(item.id) ??
        toolHistory.get(referencedToolID)
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
  const candidate = (body.input ?? [])
    .filter((item) => item.role === "system")
    .map((item) => text(item.content).match(/(?:^|\n)\s*Working directory:\s*([^\r\n<]+)/)?.[1]?.trim())
    .find((path) => path && isAbsolute(path))
  if (!candidate || !isAbsolute(candidate)) return fallback
  return resolve(candidate)
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

const providerOptions = (body, apiKey, directory, logger) => ({
  apiKey,
  cwd: directory,
  logger,
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

const usageFromFinish = (usage) => {
  const input =
    usage?.inputTokens?.total ??
    (usage?.inputTokens?.noCache ?? 0) +
      (usage?.inputTokens?.cacheRead ?? 0) +
      (usage?.inputTokens?.cacheWrite ?? 0)
  const output = usage?.outputTokens?.total ?? 0
  return {
    input_tokens: input,
    input_tokens_details: {
      cached_tokens: usage?.inputTokens?.cacheRead ?? 0,
      cache_write_tokens: usage?.inputTokens?.cacheWrite ?? 0,
    },
    output_tokens: output,
    output_tokens_details: { reasoning_tokens: usage?.outputTokens?.reasoning ?? 0 },
    total_tokens: input + output,
  }
}

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

const tokenMatches = (provided, expected) => {
  if (typeof provided !== "string") return false
  const candidate = Buffer.from(provided)
  const secret = Buffer.from(expected)
  return candidate.length === secret.length && timingSafeEqual(candidate, secret)
}

const requestSession = (request) =>
  ["x-session-id", "x-session-affinity", "x-opencode-session"]
    .map((name) => request.headers[name])
    .find((value) => typeof value === "string" && value.length > 0)

const waitForDrain = (response) =>
  new Promise((resolve, reject) => {
    const cleanup = () => {
      response.off("drain", drained)
      response.off("close", closed)
      response.off("error", failed)
    }
    const drained = () => {
      cleanup()
      resolve()
    }
    const closed = () => {
      cleanup()
      const error = new Error("client disconnected")
      error.name = "AbortError"
      reject(error)
    }
    const failed = (error) => {
      cleanup()
      reject(error)
    }
    response.once("drain", drained)
    response.once("close", closed)
    response.once("error", failed)
  })

const createSseWriter = (response, heartbeatIntervalMs) => {
  let pending = Promise.resolve()
  const write = (chunk) => {
    pending = pending.then(async () => {
      if (response.destroyed || response.writableEnded) {
        const error = new Error("client disconnected")
        error.name = "AbortError"
        throw error
      }
      if (!response.write(chunk)) await waitForDrain(response)
    })
    return pending
  }
  const heartbeat =
    heartbeatIntervalMs > 0
      ? setInterval(() => {
          void write(": heartbeat\n\n").catch(() => {})
        }, heartbeatIntervalMs)
      : undefined
  heartbeat?.unref?.()
  const stop = () => clearInterval(heartbeat)
  return {
    event: (event) => write(`data: ${JSON.stringify(event)}\n\n`),
    stop,
    drain: () => pending,
    close: async () => {
      stop()
      await pending
    },
  }
}

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
  rendered = redactSecrets(rendered)
  const limit = 16 * 1024
  return rendered.length <= limit ? rendered : `${rendered.slice(0, limit)}\n… [tool output truncated]`
}

// Best-effort scrubbing of obvious secrets before tool summaries enter
// reasoning output. Tool history is intentionally left unredacted (it feeds
// Cursor's transcript for correctness); only the human-visible summary is
// sanitized. This is defense-in-depth, not a guarantee.
const redactSecrets = (rendered) =>
  rendered
    // PEM-encoded private keys, which may span multiple lines.
    .replace(
      /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----/g,
      "[redacted]",
    )
    // Authorization headers such as `Authorization: Bearer <token>`.
    .replace(
      /(["']?)authorization\1\s*[:=]\s*["']?(?:\s*bearer\s+)?[^\s"',}]+["']?/gi,
      "$1authorization$1: [redacted]",
    )
    // Standalone bearer tokens.
    .replace(/\bBearer\s+[A-Za-z0-9._~+/=-]+/g, "Bearer [redacted]")
    // Named secret fields: `"api_key": "value"`, `token=value`, and friends.
    .replace(
      /((["']?)\b(?:api[_-]?key|apikey|access[_-]?key(?:_?id)?|secret(?:_?key)?|password|passwd|token|private[_-]?key|client[_-]?secret|refresh[_-]?token|credential)\b\2)\s*[:=]\s*["']?[^"',\s}]+["']?/gi,
      "$1: [redacted]",
    )
    // High-signal token prefixes: GitHub, OpenAI, Slack, AWS, and JWTs.
    .replace(/\b(?:gh[pousr]_|github_pat_)[A-Za-z0-9_]+/g, "[redacted]")
    .replace(/\bsk-[A-Za-z0-9_-]+/g, "[redacted]")
    .replace(/\bxox[baprs]-[A-Za-z0-9-]+/g, "[redacted]")
    .replace(/\b(?:AKIA|ASIA|AIDA|ABIA|ACCA)[A-Z0-9]{16}\b/g, "[redacted]")
    .replace(/\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g, "[redacted]")

const startReasoning = async (writer, id, outputIndex, initialText) => {
  await writer.event({
    type: "response.output_item.added",
    output_index: outputIndex,
    item: { type: "reasoning", id, summary: [], encrypted_content: null },
  })
  await writer.event({
    type: "response.reasoning_summary_part.added",
    item_id: id,
    output_index: outputIndex,
    summary_index: 0,
  })
  if (initialText) {
    await writer.event({
      type: "response.reasoning_summary_text.delta",
      item_id: id,
      output_index: outputIndex,
      summary_index: 0,
      delta: initialText,
    })
  }
}

const reasoningItem = (id, text) => ({
  type: "reasoning",
  id,
  summary: [{ type: "summary_text", text }],
  encrypted_content: null,
})

const finishReasoning = async (writer, id, outputIndex, text) => {
  await writer.event({
    type: "response.reasoning_summary_part.done",
    item_id: id,
    output_index: outputIndex,
    summary_index: 0,
  })
  await writer.event({
    type: "response.output_item.done",
    output_index: outputIndex,
    item: reasoningItem(id, text),
  })
}

const emitReasoning = async (writer, id, outputIndex, text) => {
  await startReasoning(writer, id, outputIndex, text)
  await finishReasoning(writer, id, outputIndex, text)
  return reasoningItem(id, text)
}

const startToolSummary = async (writer, part, outputIndex) => {
  const id = `cursor_tool_${part.toolCallId}`
  const summary = `Cursor tool ${part.toolName} started\nInput: ${displayValue(part.input)}`
  const item = await emitReasoning(writer, id, outputIndex, summary)
  return { id, outputIndex, name: part.toolName, input: part.input, item }
}

const finishToolSummary = async (writer, call, part, outputIndex) => {
  const id = `cursor_tool_result_${part.toolCallId}`
  const status = part.isError ? "failed" : "completed"
  const summary = `Cursor tool ${call.name} ${status}\n${part.isError ? "Error" : "Result"}: ${displayValue(part.result)}`
  const item = await emitReasoning(writer, id, outputIndex, summary)
  return { id, outputIndex, item }
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
  heartbeatIntervalMs,
  providerLogger,
}) => {
  const sdk = createQualityCursor(createCursor, providerOptions(body, apiKey, directory, providerLogger))
  const language = sdk.languageModel(body.model)
  const sessionID = body.prompt_cache_key ?? requestSessionID
  const explicitEphemeral =
    body.ephemeral === true ||
    body.metadata?.oy_ephemeral === true ||
    body.metadata?.oy_ephemeral === "true"
  const options = {
    prompt: promptFromResponses(body, toolHistory.items),
    tools: toolsFromResponses(body.tools),
    toolChoice: toolChoiceFromResponses(body.tool_choice),
    maxOutputTokens: body.max_output_tokens,
    temperature: body.temperature,
    topP: body.top_p,
    providerOptions: {
      cursor: {
        ...(sessionID ? { sessionID } : {}),
        ...(explicitEphemeral ? { ephemeral: true } : {}),
      },
    },
    abortSignal,
  }

  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
    "x-accel-buffering": "no",
  })
  response.flushHeaders?.()
  const writer = createSseWriter(response, heartbeatIntervalMs)
  try {
  const responseID = `resp_${randomBytes(12).toString("hex")}`
  const createdAt = Math.floor(Date.now() / 1_000)
  const responseMetadata = {
    id: responseID,
    object: "response",
    created_at: createdAt,
    model: body.model,
  }
  await writer.event({ type: "response.created", response: responseMetadata })
  await writer.event({ type: "response.in_progress", response: responseMetadata })

  const result = await language.doStream(options)
  const openTools = new Map()
  const openToolInputs = new Set()
  const textParts = new Map()
  const reasoning = new Map()
  const seenText = new Set()
  const seenReasoning = new Set()
  const seenToolInputs = new Set()
  const seenTools = new Set()
  const output = []
  const eventCounts = new Map()
  let nextOutputIndex = 0
  let finishUsage
  let sawStreamStart = false
  let sawFinish = false

  const count = (type) => eventCounts.set(type, (eventCounts.get(type) ?? 0) + 1)
  const allocateOutput = () => {
    const outputIndex = nextOutputIndex
    nextOutputIndex += 1
    return outputIndex
  }
  const requireOpen = (collection, id, kind) => {
    const current = collection.get(id)
    if (!current) throw new CursorBridgeProtocolError(`${kind} ${id} was not started`)
    return current
  }

  for await (const part of result.stream) {
    count(part.type)
    if (!sawStreamStart) {
      if (part.type !== "stream-start") {
        throw new CursorBridgeProtocolError(`first event was ${part.type}, expected stream-start`)
      }
      sawStreamStart = true
      continue
    }
    if (part.type === "stream-start") {
      throw new CursorBridgeProtocolError("received more than one stream-start event")
    }
    if (sawFinish) {
      throw new CursorBridgeProtocolError(`received ${part.type} after finish`)
    }
    if (part.type === "text-start") {
      if (seenText.has(part.id)) {
        throw new CursorBridgeProtocolError(`text ${part.id} started more than once`)
      }
      seenText.add(part.id)
      const outputIndex = allocateOutput()
      textParts.set(part.id, { outputIndex, text: "" })
      await writer.event({
        type: "response.output_item.added",
        output_index: outputIndex,
        item: { type: "message", id: part.id, phase: null, role: "assistant", content: [] },
      })
      continue
    }
    if (part.type === "text-delta") {
      const current = requireOpen(textParts, part.id, "text")
      current.text += part.delta
      await writer.event({
        type: "response.output_text.delta",
        item_id: part.id,
        output_index: current.outputIndex,
        delta: part.delta,
      })
      continue
    }
    if (part.type === "text-end") {
      const current = requireOpen(textParts, part.id, "text")
      textParts.delete(part.id)
      const item = {
        type: "message",
        id: part.id,
        phase: null,
        status: "completed",
        role: "assistant",
        content: [{ type: "output_text", text: current.text, annotations: [] }],
      }
      await writer.event({
        type: "response.output_text.done",
        item_id: part.id,
        output_index: current.outputIndex,
        text: current.text,
      })
      await writer.event({
        type: "response.output_item.done",
        output_index: current.outputIndex,
        item,
      })
      output[current.outputIndex] = item
      continue
    }
    if (part.type === "reasoning-start") {
      if (seenReasoning.has(part.id)) {
        throw new CursorBridgeProtocolError(`reasoning ${part.id} started more than once`)
      }
      seenReasoning.add(part.id)
      const outputIndex = allocateOutput()
      reasoning.set(part.id, { outputIndex, text: "" })
      await startReasoning(writer, part.id, outputIndex, "")
      continue
    }
    if (part.type === "reasoning-delta") {
      const current = requireOpen(reasoning, part.id, "reasoning")
      current.text += part.delta
      await writer.event({
        type: "response.reasoning_summary_text.delta",
        item_id: part.id,
        output_index: current.outputIndex,
        summary_index: 0,
        delta: part.delta,
      })
      continue
    }
    if (part.type === "reasoning-end") {
      const current = requireOpen(reasoning, part.id, "reasoning")
      reasoning.delete(part.id)
      await finishReasoning(writer, part.id, current.outputIndex, current.text)
      output[current.outputIndex] = reasoningItem(part.id, current.text)
      continue
    }
    if (part.type === "tool-input-start") {
      if (seenToolInputs.has(part.id)) {
        throw new CursorBridgeProtocolError(`tool input ${part.id} started more than once`)
      }
      seenToolInputs.add(part.id)
      openToolInputs.add(part.id)
      continue
    }
    if (part.type === "tool-input-delta") {
      if (!openToolInputs.has(part.id)) {
        throw new CursorBridgeProtocolError(`tool input ${part.id} received a delta before start`)
      }
      continue
    }
    if (part.type === "tool-input-end") {
      if (!openToolInputs.delete(part.id)) {
        throw new CursorBridgeProtocolError(`tool input ${part.id} ended before start`)
      }
      continue
    }
    if (part.type === "tool-call") {
      if (seenTools.has(part.toolCallId)) {
        throw new CursorBridgeProtocolError(`tool ${part.toolCallId} started more than once`)
      }
      seenTools.add(part.toolCallId)
      if (openToolInputs.has(part.toolCallId)) {
        throw new CursorBridgeProtocolError(`tool ${part.toolCallId} was called before its input ended`)
      }
      const call = await startToolSummary(writer, part, allocateOutput())
      openTools.set(part.toolCallId, call)
      output[call.outputIndex] = call.item
      continue
    }
    if (part.type === "tool-result") {
      const call = requireOpen(openTools, part.toolCallId, "tool")
      openTools.delete(part.toolCallId)
      rememberTool(toolHistory, part.toolCallId, call, part.result)
      const result = await finishToolSummary(writer, call, part, allocateOutput())
      output[result.outputIndex] = result.item
      continue
    }
    if (part.type === "error") throw part.error
    if (part.type === "finish") {
      if (part.finishReason?.unified !== "stop") {
        throw new CursorBridgeProtocolError(
          `finish reason was ${part.finishReason?.unified ?? "missing"}, expected stop`,
        )
      }
      finishUsage = usageFromFinish(part.usage)
      sawFinish = true
      continue
    }
    throw new CursorBridgeProtocolError(`unsupported event ${part.type}`)
  }
  if (!sawFinish) throw new CursorBridgeProtocolError("stream ended without finish")
  if (textParts.size > 0) {
    throw new CursorBridgeProtocolError(`stream finished with ${textParts.size} open text item(s)`)
  }
  if (reasoning.size > 0) {
    throw new CursorBridgeProtocolError(`stream finished with ${reasoning.size} open reasoning item(s)`)
  }
  if (openToolInputs.size > 0) {
    throw new CursorBridgeProtocolError(`stream finished with ${openToolInputs.size} open tool input(s)`)
  }
  if (openTools.size > 0) {
    throw new CursorBridgeProtocolError(`stream finished with ${openTools.size} open tool call(s)`)
  }
  if (output.filter(Boolean).length !== nextOutputIndex) {
    throw new CursorBridgeProtocolError("stream finished without completing every output item")
  }

  const usage = finishUsage ?? usageFromFinish()
  await writer.event({
    type: "response.completed",
    response: {
      ...responseMetadata,
      status: "completed",
      output,
      usage,
    },
  })
  await writer.close()
  response.end()
  return {
    responseID,
    events: Object.fromEntries(eventCounts),
    outputItems: output.length,
    usage,
  }
  } finally {
    writer.stop()
    await writer.drain().catch(() => {})
  }
}

export const startCursorBridge = async ({
  directory,
  createCursor,
  listCursorModels,
  onModels,
  onDirectory,
  onIdle,
  reportError,
  reportDiagnostic = () => {},
  heartbeatIntervalMs = cursorBridgeLimits.heartbeatMs,
  modelRefreshTimeoutMs = cursorBridgeLimits.modelRefreshMs,
  modelRefreshRetryMs = cursorBridgeLimits.modelRefreshRetryMs,
}) => {
  const handlers = {
    createCursor,
    listCursorModels,
    onModels,
    onDirectory,
    onIdle,
    reportError,
    reportDiagnostic,
  }
  const existing = bridges.get(directory)
  if (existing) {
    if (!existing.listening) await existing.ready
    if (bridges.get(directory) !== existing) {
      return startCursorBridge({
        directory,
        createCursor,
        listCursorModels,
        onModels,
        onDirectory,
        onIdle,
        reportError,
        reportDiagnostic,
        heartbeatIntervalMs,
        modelRefreshTimeoutMs,
        modelRefreshRetryMs,
      })
    }
    return createBridgeLease(existing, handlers)
  }

  const token = randomBytes(32).toString("base64url")
  const refreshedKeys = new Set()
  const modelRefreshes = new Map()
  const toolHistories = new Map()
  const active = new Map()
  const activeRequests = new Set()
  let resolveReady
  let rejectReady
  const ready = new Promise((resolve, reject) => {
    resolveReady = resolve
    rejectReady = reject
  })
  void ready.catch(() => {})
  const state = {
    directory,
    current: handlers,
    owners: [],
    closing: undefined,
    activeRequests,
    listening: false,
    ready,
  }
  const shortHash = (value) => createHash("sha256").update(value).digest("hex").slice(0, 12)
  const refreshModels = (fingerprint, apiKey, context) => {
    if (refreshedKeys.has(fingerprint)) return
    const refresh = modelRefreshes.get(fingerprint) ?? { retryAt: 0, running: undefined }
    if (refresh.running || refresh.retryAt > Date.now()) return
    refresh.running = (async () => {
      const models = await withTimeout(
        state.current.listCursorModels(apiKey),
        modelRefreshTimeoutMs,
        "Cursor model refresh",
      )
      await state.current.onModels(models)
      refreshedKeys.add(fingerprint)
      modelRefreshes.delete(fingerprint)
    })()
      .catch((error) => {
        refresh.retryAt = Date.now() + modelRefreshRetryMs
        state.current.reportError("failed to refresh Cursor models from bridge", error, context)
      })
      .finally(() => {
        refresh.running = undefined
      })
    modelRefreshes.set(fingerprint, refresh)
  }
  const providerLogger = (level, message, extra) => {
    if (level === "warn" || level === "error") {
      state.current.reportError(
        `Cursor provider ${level}`,
        new Error(message),
        extra,
      )
      return
    }
    state.current.reportDiagnostic("Cursor provider diagnostic", { level, message, ...extra })
  }
  const server = createServer(async (request, response) => {
    if (request.method !== "POST" || !request.url?.endsWith("/responses")) {
      response.writeHead(404).end()
      return
    }
    if (!tokenMatches(request.headers["x-oy-cursor-bridge"], token)) {
      response.writeHead(403).end()
      return
    }
    const apiKey = bearer(request)
    if (!apiKey) {
      response.writeHead(401).end()
      return
    }
    const startedAt = Date.now()
    const requestID = `bridge_${randomBytes(8).toString("hex")}`
    let requestContext = { requestID }
    try {
      const body = await readBody(request)
      const cwd = requestDirectory(body, directory)
      requestContext = {
        requestID,
        model: typeof body.model === "string" ? body.model : "unknown",
        cwd: shortHash(cwd),
      }
      state.current.onDirectory(cwd)
      active.set(cwd, (active.get(cwd) ?? 0) + 1)
      const fingerprint = createHash("sha256").update(apiKey).digest("hex")
      const session = body.prompt_cache_key ?? requestSession(request) ?? "ephemeral"
      requestContext.session = shortHash(session)
      const historyKey = `${fingerprint}:${cwd}:${session}`
      const toolHistory = historyFor(toolHistories, historyKey)
      refreshModels(fingerprint, apiKey, requestContext)
      try {
        const abort = new AbortController()
        activeRequests.add(abort)
        const abortRequest = () => abort.abort()
        request.once("aborted", abortRequest)
        response.once("close", abortRequest)
        try {
          const result = await streamCursor({
            body,
            apiKey,
            directory: cwd,
            createCursor: state.current.createCursor,
            response,
            toolHistory,
            abortSignal: abort.signal,
            requestSessionID: requestSession(request),
            heartbeatIntervalMs,
            providerLogger,
          })
          state.current.reportDiagnostic("Cursor bridge request completed", {
            ...requestContext,
            durationMs: Date.now() - startedAt,
            ...result,
          })
        } finally {
          activeRequests.delete(abort)
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
      const aborted = error?.name === "AbortError"
      if (!aborted || !response.destroyed) {
        state.current.reportError("Cursor bridge request failed", error, {
          ...requestContext,
          durationMs: Date.now() - startedAt,
          code: error?.code,
          status: error?.status,
          updates: error?.updates,
        })
      }
      if (response.writableEnded) return
      if (!response.headersSent) {
        response.writeHead(400, { "content-type": "application/json" })
        response.end(JSON.stringify({ error: "Cursor bridge request failed" }))
        return
      }
      if (!response.destroyed) {
        response.write(
          `data: ${JSON.stringify({
            type: "error",
            sequence_number: 0,
            code: error?.code ?? "cursor_bridge_error",
            message: error instanceof Error ? error.message : String(error),
            param: null,
          })}\n\n`,
        )
      }
      response.end()
    }
  })
  bridges.set(directory, state)
  try {
    await new Promise((resolve, reject) => {
      const failed = (error) => {
        server.off("listening", listening)
        reject(error)
      }
      const listening = () => {
        server.off("error", failed)
        resolve()
      }
      server.once("error", failed)
      server.once("listening", listening)
      server.listen(0, "127.0.0.1")
    })
    server.on("error", (error) => state.current.reportError("Cursor bridge server failed", error))
    server.unref()
    const address = server.address()
    if (!address || typeof address === "string") {
      throw new Error("Cursor bridge did not bind a TCP port")
    }
    Object.assign(state, {
      listening: true,
      url: `http://127.0.0.1:${address.port}/v1`,
      token,
      server,
      clear: () => {
        refreshedKeys.clear()
        modelRefreshes.clear()
        toolHistories.clear()
      },
    })
    resolveReady()
  } catch (error) {
    if (bridges.get(directory) === state) bridges.delete(directory)
    if (server.listening) server.close()
    rejectReady(error)
    throw error
  }
  return createBridgeLease(state, handlers)
}

const closeBridgeServer = (state) =>
  new Promise((resolve, reject) => {
    let settled = false
    let force
    const finish = (error) => {
      if (settled) return
      settled = true
      clearTimeout(force)
      clearTimeout(timeout)
      if (error && error.code !== "ERR_SERVER_NOT_RUNNING") reject(error)
      else resolve()
    }
    const timeout = setTimeout(() => {
      state.server.closeAllConnections?.()
      finish()
    }, cursorBridgeLimits.shutdownMs)
    timeout.unref?.()
    state.server.closeIdleConnections?.()
    state.server.close(finish)
    force = setTimeout(
      () => state.server.closeAllConnections?.(),
      cursorBridgeLimits.shutdownForceMs,
    )
    force.unref?.()
  })

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
      for (const controller of state.activeRequests) controller.abort()
      state.clear()
      state.closing ??= closeBridgeServer(state)
      return state.closing
    },
  }
}
