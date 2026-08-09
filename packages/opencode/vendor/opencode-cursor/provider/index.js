// src/provider/index.ts
import { NoSuchModelError } from "@ai-sdk/provider";

// src/api-key.ts
import { createHash } from "crypto";
var CURSOR_API_KEY_ENV_VAR = "CURSOR_API_KEY";
var PLACEHOLDERS = /* @__PURE__ */ new Set([
  CURSOR_API_KEY_ENV_VAR,
  `$${CURSOR_API_KEY_ENV_VAR}`,
  `\${${CURSOR_API_KEY_ENV_VAR}}`
]);
function resolveCursorApiKey(candidate) {
  const trimmed = candidate?.trim();
  if (trimmed && !PLACEHOLDERS.has(trimmed)) return trimmed;
  const fromEnv = process.env[CURSOR_API_KEY_ENV_VAR]?.trim();
  return fromEnv ? fromEnv : void 0;
}

// src/provider/agent-backend.ts
import { execSync } from "child_process";
import { existsSync } from "fs";
import { fileURLToPath } from "url";

// src/cursor-runtime.ts
var cached;
async function loadCursorSdk() {
  if (!cached) {
    cached = import("@cursor/sdk").catch((err) => {
      cached = void 0;
      const detail = err instanceof Error ? err.message : String(err);
      throw new Error(
        `[opencode-cursor] Failed to load "@cursor/sdk". Make sure it is installed (\`npm install @cursor/sdk\`). Original error: ${detail}`
      );
    });
  }
  return cached;
}

// src/provider/log-bridge.ts
import { AsyncLocalStorage } from "async_hooks";
var loggerContext = new AsyncLocalStorage();
function withProviderLogger(logger, operation) {
  return logger ? loggerContext.run(logger, operation) : operation();
}
function getProviderLogger() {
  return loggerContext.getStore();
}
var SERVICE = "opencode-cursor";
function pluginLog(level, message, extra, logger = getProviderLogger()) {
  if (logger) {
    try {
      logger(level, message, extra);
    } catch {
    }
    return;
  }
  if (level === "debug" && process.env["OPENCODE_CURSOR_DEBUG"] !== "1") return;
  const line = extra ? `[${SERVICE}] ${message} ${JSON.stringify(extra)}` : `[${SERVICE}] ${message}`;
  switch (level) {
    case "debug":
      console.debug(line);
      break;
    case "info":
      console.info(line);
      break;
    case "warn":
      console.warn(line);
      break;
    case "error":
      console.error(line);
      break;
  }
}

// src/provider/cursor-log-intercept.ts
var ANSI_PATTERN = /\x1b\[[0-9;]*m/g;
function stripAnsi(input) {
  return input.replace(ANSI_PATTERN, "");
}
var RULE_LOAD_PATTERN = /^\d{2}:\d{2}:\d{2}\.\d{3}\s+INFO\s+(LocalCursorRulesService|AgentSkillsCursorRulesService|CursorPluginsAgentSkillsService) load completed(?:\s+ctx=\S+)?\s+meta=\{([^}]*)\}\s*$/;
function parseCursorLogMeta(raw) {
  const out = {};
  for (const part of raw.split(",")) {
    const [key, value] = part.split(":").map((s) => s.trim());
    if (!key || value === void 0) continue;
    const num = Number(value);
    if (Number.isFinite(num)) out[key] = num;
  }
  return out;
}
function parseCursorRuleLoadLine(line) {
  const match = RULE_LOAD_PATTERN.exec(stripAnsi(line));
  if (!match) return void 0;
  const [, service, meta] = match;
  if (!service) return void 0;
  return { service, meta: parseCursorLogMeta(meta ?? "") };
}
var installed = false;
var original;
function installCursorLogInterceptor() {
  if (installed) return;
  original = console.log.bind(console);
  const passthrough = original;
  console.log = (...args) => {
    if (args.length === 1 && typeof args[0] === "string") {
      const parsed = parseCursorRuleLoadLine(args[0]);
      if (parsed) {
        pluginLog("info", `${parsed.service} load completed`, parsed.meta);
        return;
      }
    }
    passthrough(...args);
  };
  installed = true;
}

// src/provider/sidecar-client.ts
import { spawn } from "child_process";
import { createInterface } from "readline";
function reviveError(error) {
  const e = error ?? {};
  const err = new Error(e.message ?? "sidecar error");
  if (e.name) err.name = e.name;
  if (e.status !== void 0) err.status = e.status;
  if (e.code !== void 0) err.code = e.code;
  if (e.isRetryable !== void 0) err.isRetryable = e.isRetryable;
  if (e.helpUrl !== void 0) err.helpUrl = e.helpUrl;
  return err;
}
var SidecarClient = class {
  options;
  child;
  reader;
  pending = /* @__PURE__ */ new Map();
  nextId = 1;
  disposed = false;
  constructor(options) {
    this.options = options;
  }
  /** Spawn (or reuse) the child process. */
  ensureChild() {
    if (this.disposed) throw new Error("cursor sidecar client disposed");
    if (this.child) return this.child;
    const child = spawn(this.options.nodePath ?? "node", [this.options.scriptPath], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...this.options.env }
    });
    this.child = child;
    this.reader = createInterface({ input: child.stdout });
    this.reader.on("line", (line) => this.handleLine(line));
    child.stderr.on("data", (chunk) => {
      if (this.options.debug || process.env["OPENCODE_CURSOR_DEBUG"]) {
        process.stderr.write(`[cursor:sidecar] ${chunk}`);
      }
    });
    child.on("exit", (code) => {
      this.failAll(new Error(`cursor sidecar exited (code ${code ?? "unknown"})`));
      this.child = void 0;
      this.reader?.close();
      this.reader = void 0;
    });
    child.on("error", (err) => {
      this.failAll(new Error(`cursor sidecar failed to start: ${err.message}`));
      this.child = void 0;
    });
    this.updateRefs();
    return child;
  }
  /**
   * Keep the child (and its pipes) from holding the parent's event loop open
   * while idle, but ref it whenever a reply is outstanding so the loop can't
   * exit mid-request. Without this, any process that uses the provider and
   * never dispose()s — scripts, tests, opencode itself on shutdown — hangs.
   */
  updateRefs() {
    const child = this.child;
    if (!child) return;
    const refable = [child, child.stdin, child.stdout, child.stderr];
    if (this.pending.size > 0) {
      for (const target of refable) target.ref?.();
    } else {
      for (const target of refable) target.unref?.();
    }
  }
  failAll(err) {
    for (const pending of this.pending.values()) {
      pending.onStreamError?.(err);
      pending.reject(err);
    }
    this.pending.clear();
    this.updateRefs();
  }
  handleLine(line) {
    if (!line.trim()) return;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (msg["ev"] === "log") {
      const level = msg["level"];
      const message = msg["message"];
      if (typeof level === "string" && typeof message === "string") {
        this.options.onLog?.(
          level,
          message,
          msg["meta"]
        );
      }
      return;
    }
    const id = msg["id"];
    if (typeof id !== "number") return;
    const pending = this.pending.get(id);
    if (!pending) return;
    const ev = msg["ev"];
    if (ev === "update") {
      pending.onUpdate?.(msg["update"]);
      return;
    }
    if (ev === "result") {
      this.pending.delete(id);
      this.updateRefs();
      pending.onResult?.(msg["result"]);
      return;
    }
    if (ev === "error") {
      this.pending.delete(id);
      this.updateRefs();
      pending.onStreamError?.(reviveError(msg["error"]));
      return;
    }
    if (msg["ok"] === true) {
      if (!pending.onResult) {
        this.pending.delete(id);
        this.updateRefs();
      }
      pending.resolve(msg);
    } else {
      this.pending.delete(id);
      this.updateRefs();
      pending.reject(reviveError(msg["error"]));
    }
  }
  request(payload, hooks) {
    const child = this.ensureChild();
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, ...hooks });
      this.updateRefs();
      child.stdin.write(`${JSON.stringify({ id, ...payload })}
`, (err) => {
        if (err) {
          this.pending.delete(id);
          this.updateRefs();
          reject(err);
        }
      });
    });
  }
  async createAgent(options) {
    const res = await this.request({ op: "create", options });
    return this.wrapAgent(String(res["agentId"]));
  }
  async resumeAgent(agentId, options) {
    const res = await this.request({ op: "resume", agentId, options });
    return this.wrapAgent(String(res["agentId"]));
  }
  wrapAgent(agentId) {
    return {
      agentId,
      send: (message, options) => this.sendTurn(agentId, message, options),
      close: () => {
        void this.request({ op: "close", agentId }).catch(() => {
        });
      }
    };
  }
  async sendTurn(agentId, message, options) {
    let settle;
    const waited = new Promise((resolve, reject) => {
      settle = { resolve, reject };
    });
    waited.catch(() => {
    });
    let sendId;
    const ack = this.request(
      {
        op: "send",
        agentId,
        message,
        ...options?.mode ? { mode: options.mode } : {},
        ...options?.local?.force ? { force: true } : {},
        ...options?.idempotencyKey ? { idempotencyKey: options.idempotencyKey } : {}
      },
      {
        onUpdate: (update) => options?.onDelta?.({ update }),
        onResult: (result) => settle.resolve(result),
        onStreamError: (err) => settle.reject(err)
      }
    );
    sendId = this.nextId - 1;
    await ack;
    return {
      wait: () => waited,
      cancel: async () => {
        if (sendId === void 0) return;
        await this.request({ op: "cancel", sendId }).catch(() => {
        });
      }
    };
  }
  /** Kill the child and reject anything in flight. */
  dispose() {
    this.disposed = true;
    this.failAll(new Error("cursor sidecar client disposed"));
    this.reader?.close();
    this.reader = void 0;
    this.child?.kill();
    this.child = void 0;
  }
};

// src/provider/agent-backend.ts
var DEFAULT_BUN_TRANSPORT = "http1";
var preferredTransport;
function setPreferredTransport(t) {
  preferredTransport = t;
}
function isTransportKind(v) {
  return v === "http1" || v === "http2-direct" || v === "sidecar";
}
function resolveTransport(env) {
  const requested = preferredTransport ?? (isTransportKind(process.env["OPENCODE_CURSOR_TRANSPORT"]) ? process.env["OPENCODE_CURSOR_TRANSPORT"] : void 0);
  if (requested) {
    if (requested === "sidecar" && !env.nodePath) {
      return env.isBun ? "http1" : "http2-direct";
    }
    return requested;
  }
  const legacy = process.env["OPENCODE_CURSOR_SIDECAR"];
  if (legacy === "1" || legacy === "true") {
    return env.nodePath ? "sidecar" : env.isBun ? "http1" : "http2-direct";
  }
  if (legacy === "0" || legacy === "false") return env.isBun ? "http1" : "http2-direct";
  return env.isBun ? DEFAULT_BUN_TRANSPORT : "http2-direct";
}
function detectNode() {
  try {
    const out = execSync(process.platform === "win32" ? "where node" : "command -v node", {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"]
    }).trim();
    return out.split("\n")[0] || void 0;
  } catch {
    return void 0;
  }
}
function detectEnvironment() {
  const isBun = typeof globalThis.Bun !== "undefined";
  const needsNode = isBun || process.env["OPENCODE_CURSOR_SIDECAR"] === "1";
  return { isBun, nodePath: needsNode ? detectNode() : process.execPath };
}
var http1Configured = false;
async function ensureHttp1Configured() {
  if (http1Configured) return;
  const { Cursor } = await loadCursorSdk();
  Cursor.configure({ local: { useHttp1ForAgent: true } });
  http1Configured = true;
}
function inProcessBackend(useHttp1) {
  installCursorLogInterceptor();
  return {
    kind: "in-process",
    createAgent: async (options) => {
      const { Agent } = await loadCursorSdk();
      if (useHttp1) await ensureHttp1Configured();
      return await Agent.create(options);
    },
    resumeAgent: async (agentId, options) => {
      const { Agent } = await loadCursorSdk();
      if (useHttp1) await ensureHttp1Configured();
      return await Agent.resume(agentId, options);
    }
  };
}
function resolveSidecarScript() {
  const candidates = [
    "./sidecar/agent-host.js",
    // importer is a chunk at dist root
    "../sidecar/agent-host.js",
    // importer is dist/provider/index.js
    "../sidecar/agent-host.mjs"
    // importer is src/provider/*.ts (dev/tests)
  ];
  for (const candidate of candidates) {
    const path = fileURLToPath(new URL(candidate, import.meta.url));
    if (existsSync(path)) return path;
  }
  return void 0;
}
function sidecarBackend(nodePath, scriptPath, logger) {
  const client = new SidecarClient({
    scriptPath,
    nodePath,
    onLog: (level, message, meta) => pluginLog(level, message, meta, logger)
  });
  return {
    kind: "sidecar",
    createAgent: (options) => client.createAgent(options),
    resumeAgent: (agentId, options) => client.resumeAgent(agentId, options)
  };
}
var cached2;
var cachedByLogger = /* @__PURE__ */ new WeakMap();
function loadAgentBackend(logger) {
  let current = logger ? cachedByLogger.get(logger) : cached2;
  if (!current) {
    const env = detectEnvironment();
    const transport = resolveTransport(env);
    const scriptPath = transport === "sidecar" ? resolveSidecarScript() : void 0;
    if (transport === "sidecar" && (!env.nodePath || !scriptPath)) {
      pluginLog(
        "warn",
        "Node sidecar requested but unavailable; falling back to in-process HTTP/1.1 transport.",
        { node: env.nodePath ?? null, script: scriptPath ?? null },
        logger
      );
      current = inProcessBackend(true);
    } else {
      if (transport === "http2-direct" && env.isBun) {
        pluginLog(
          "warn",
          "http2-direct under Bun: Cursor streams may fail (Bun node:http2 incompatibility, oven-sh/bun#31499). Set OPENCODE_CURSOR_TRANSPORT=http1 (recommended) or sidecar.",
          void 0,
          logger
        );
      }
      current = transport === "sidecar" && env.nodePath && scriptPath ? sidecarBackend(env.nodePath, scriptPath, logger) : inProcessBackend(transport === "http1");
    }
    if (logger) cachedByLogger.set(logger, current);
    else cached2 = current;
  }
  return current;
}

// src/provider/language-model.ts
import { LoadAPIKeyError } from "@ai-sdk/provider";

// src/provider/message-map.ts
import { fileURLToPath as fileURLToPath2 } from "url";
var TOOL_RESULT_CAP = 2e3;
var TOOL_ARGS_CAP = 500;
function stringify(value) {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value ?? null);
  } catch {
    return String(value);
  }
}
function truncate(text, cap) {
  return text.length > cap ? `${text.slice(0, cap)}\u2026[+${text.length - cap} chars]` : text;
}
function promptToCursorMessage(prompt, systemPrompt = "rules") {
  const lines = [];
  prompt.forEach((message) => {
    switch (message.role) {
      case "system":
        if (systemPrompt === "message") {
          lines.push(`# System
${message.content}`);
        }
        break;
      case "user": {
        const text = [];
        for (const part of message.content) {
          if (part.type === "text") text.push(part.text);
          else if (part.type === "file") text.push(fileNote(part));
        }
        lines.push(`# User
${text.join("\n")}`);
        break;
      }
      case "assistant": {
        const text = [];
        for (const part of message.content) {
          if (part.type === "text") text.push(part.text);
          else if (part.type === "reasoning")
            text.push(`(thinking) ${part.text}`);
          else if (part.type === "tool-call")
            text.push(
              `[called ${part.toolName}(${truncate(stringify(part.input), TOOL_ARGS_CAP)})]`
            );
          else if (part.type === "tool-result")
            text.push(
              `[result of ${part.toolName}: ${truncate(stringify(part.output), TOOL_RESULT_CAP)}]`
            );
        }
        lines.push(`# Assistant
${text.join("\n")}`);
        break;
      }
      case "tool": {
        for (const part of message.content) {
          if (part.type === "tool-result") {
            lines.push(
              `# Tool result (${part.toolName})
${truncate(stringify(part.output), TOOL_RESULT_CAP)}`
            );
          }
        }
        break;
      }
    }
  });
  return { text: lines.join("\n\n") };
}
function fileNote(part) {
  const name = part.filename ?? describeSource(part.data) ?? "file";
  return `[attached file: ${name} (${part.mediaType}) \u2014 not forwarded to Cursor]`;
}
var URL_SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;
function describeSource(data) {
  if (data instanceof URL)
    return data.protocol === "file:" ? fileUrlToPath(data) : data.href;
  if (typeof data === "string" && URL_SCHEME.test(data))
    return data.startsWith("file://") ? fileUrlToPath(data) : data;
  return void 0;
}
function fileUrlToPath(url) {
  try {
    return fileURLToPath2(url);
  } catch {
    return typeof url === "string" ? url : url.href;
  }
}
function userTurnToCursorMessage(message) {
  const text = [];
  for (const part of message.content) {
    if (part.type === "text") text.push(part.text);
    else if (part.type === "file") text.push(fileNote(part));
  }
  return { text: text.join("\n") };
}
function latestUserMessage(prompt) {
  const last = prompt[prompt.length - 1];
  if (!last || last.role !== "user") return void 0;
  return userTurnToCursorMessage(last);
}
function trailingUserMessages(prompt, count) {
  if (count <= 0) return [];
  const collected = [];
  for (let i = prompt.length - 1; i >= 0 && collected.length < count; i--) {
    const message = prompt[i];
    if (!message || message.role !== "user") break;
    collected.push(userTurnToCursorMessage(message));
  }
  return collected.reverse();
}

// src/provider/system-rule.ts
import {
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync as existsSync2,
  rmSync
} from "fs";
import { join } from "path";
var RULES_DIR = join(".cursor", "rules");
var RULE_FILE = "opencode.mdc";
var IGNORE_FILE = ".gitignore";
var SENTINEL = "generated: opencode-cursor";
function extractSystemText(prompt) {
  const parts = [];
  for (const message of prompt) {
    if (message.role === "system") parts.push(message.content);
  }
  return parts.join("\n\n").trim();
}
function isGenerated(content) {
  if (!content.startsWith("---")) return false;
  const end = content.indexOf("\n---", 3);
  const frontmatter = end === -1 ? content : content.slice(0, end);
  return frontmatter.split(/\r?\n/).includes(SENTINEL);
}
function writeSystemRule(cwd, systemText, skillsCatalogue) {
  if (!systemText && !skillsCatalogue) return "empty";
  const dir = join(cwd, RULES_DIR);
  const path = join(dir, RULE_FILE);
  const sections = [systemText, skillsCatalogue].filter(Boolean).join("\n\n");
  const body = `---
alwaysApply: true
${SENTINEL}
---

${sections}
`;
  const existing = existsSync2(path) ? readFileSync(path, "utf8") : void 0;
  if (existing !== void 0) {
    if (!isGenerated(existing)) return "blocked";
    if (existing === body) return "unchanged";
  }
  mkdirSync(dir, { recursive: true });
  writeFileSync(path, body, "utf8");
  ensureGitIgnored(dir);
  return "written";
}
function ensureGitIgnored(dir) {
  const path = join(dir, IGNORE_FILE);
  const existing = existsSync2(path) ? readFileSync(path, "utf8") : "";
  const lines = existing.split(/\r?\n/);
  const missing = [RULE_FILE, IGNORE_FILE].filter(
    (entry) => !lines.includes(entry)
  );
  if (missing.length === 0) return;
  const prefix = existing && !existing.endsWith("\n") ? `${existing}
` : existing;
  writeFileSync(path, `${prefix}${missing.join("\n")}
`, "utf8");
}
function resolveSystemDelivery(options) {
  const { mode, settingSources, cwd, systemText, skillsCatalogue, warn } = options;
  if (mode !== "rules") return { mode, settingSources };
  if (settingSources && !settingSources.includes("project")) {
    warn(
      'systemPrompt "rules" needs the "project" settings layer, but settingSources was explicitly configured without it; delivering the system prompt inline ("message" mode) instead. Add "project" to settingSources or set systemPrompt: "message" to silence this.'
    );
    return { mode: "message", settingSources };
  }
  let result;
  try {
    result = writeSystemRule(cwd, systemText, skillsCatalogue);
  } catch (error) {
    warn(
      `failed to write .cursor/rules/${RULE_FILE} (${error instanceof Error ? error.message : String(error)}); delivering the system prompt inline ("message" mode) for this turn.`
    );
    return { mode: "message", settingSources };
  }
  if (result === "blocked") {
    warn(
      `.cursor/rules/${RULE_FILE} exists but was not generated by opencode-cursor; leaving it untouched and delivering the system prompt inline ("message" mode).`
    );
    return { mode: "message", settingSources };
  }
  if (result === "empty") return { mode, settingSources };
  return { mode, settingSources: settingSources ?? ["project"] };
}

// src/provider/error-classify.ts
function classifyError(err) {
  const e = err ?? {};
  const name = typeof e.name === "string" ? e.name : "Error";
  const message = typeof e.message === "string" ? e.message : String(err);
  const status = typeof e.status === "number" ? e.status : void 0;
  const code = typeof e.code === "string" ? e.code : void 0;
  const helpUrl = typeof e.helpUrl === "string" ? e.helpUrl : void 0;
  const base = { ...status !== void 0 ? { status } : {}, ...helpUrl ? { helpUrl } : {}, message };
  switch (name) {
    case "AgentNotFoundError":
      return { kind: "agent-not-found", retryable: false, ...base };
    case "AgentBusyError":
      return { kind: "agent-busy", retryable: false, ...base };
    case "RateLimitError":
      return { kind: "rate-limit", retryable: true, ...base };
    case "NetworkError":
      return { kind: "network", retryable: true, ...base };
    case "AuthenticationError":
      return { kind: "auth", retryable: false, ...base };
    case "ConfigurationError":
    case "IntegrationNotConnectedError":
    case "UnsupportedRunOperationError":
      return { kind: "config", retryable: false, ...base };
  }
  if (status === 401) return { kind: "auth", retryable: false, ...base };
  if (status === 429) return { kind: "rate-limit", retryable: true, ...base };
  if (status === 409) return { kind: "agent-busy", retryable: false, ...base };
  if (status === 503 || status === 504) return { kind: "network", retryable: true, ...base };
  if (code === "agent_not_found") return { kind: "agent-not-found", retryable: false, ...base };
  return { kind: "unknown", retryable: e.isRetryable === true, ...base };
}

// src/provider/agent-events.ts
var CursorRunError = class extends Error {
  status;
  result;
  code;
  terminalError;
  updates;
  constructor(result, updates) {
    const details = [];
    const resultDetail = result.result?.trim();
    if (resultDetail) details.push(resultDetail);
    if (result.error?.message && result.error.message !== resultDetail) {
      details.push(
        `error: ${result.error.message}${result.error.code ? ` [${result.error.code}]` : ""}`
      );
    }
    const updateSummary = Object.entries(updates).sort(([a], [b]) => a.localeCompare(b)).map(([type, count]) => `${type}=${count}`).join(", ");
    super(
      `Cursor run ended with status "${result.status}"` + (details.length > 0 ? `: ${details.join("; ")}` : " with no result or error detail") + ` (${updateSummary ? `SDK updates: ${updateSummary}` : "no SDK updates received"})`
    );
    this.name = "CursorRunError";
    this.status = result.status;
    this.result = result.result;
    this.code = result.error?.code;
    this.terminalError = result.error ? Object.freeze({ ...result.error }) : void 0;
    this.updates = Object.freeze({ ...updates });
  }
};
function addUsage(a, b) {
  if (!a) return b;
  if (!b) return a;
  const hasReasoning = a.reasoningTokens !== void 0 || b.reasoningTokens !== void 0;
  return {
    inputTokens: a.inputTokens + b.inputTokens,
    outputTokens: a.outputTokens + b.outputTokens,
    cacheReadTokens: a.cacheReadTokens + b.cacheReadTokens,
    cacheWriteTokens: a.cacheWriteTokens + b.cacheWriteTokens,
    ...hasReasoning ? { reasoningTokens: (a.reasoningTokens ?? 0) + (b.reasoningTokens ?? 0) } : {}
  };
}
function toolDisplayName(toolCall) {
  if (!toolCall) return "tool";
  if (toolCall.type === "mcp") {
    const name = toolCall.args?.toolName;
    const server = toolCall.args?.providerIdentifier;
    if (name) return server ? `${server}/${name}` : String(name);
    return "mcp";
  }
  return toolCall.type ?? "tool";
}
var MAX_TIMEOUT_MS = 2147483647;
function envMs(name, fallback) {
  const raw = process.env[name];
  if (raw === void 0) return fallback;
  if (raw === "") return 0;
  const n = Number(raw);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(n, MAX_TIMEOUT_MS);
}
async function* streamAgentTurn(agent, message, options) {
  const queue = [];
  let wake;
  let finished = false;
  let failure;
  const debug = process.env.OPENCODE_CURSOR_DEBUG === "1";
  const counts = {};
  const stallMs = envMs("OPENCODE_CURSOR_STALL_MS", 12e4);
  const toolStallMs = envMs("OPENCODE_CURSOR_TOOL_STALL_MS", 6e5);
  let stallTimer;
  let forced = false;
  let anyEvent = false;
  const openTools = /* @__PURE__ */ new Map();
  const push = (event) => {
    anyEvent = true;
    queue.push(event);
    wake?.();
    wake = void 0;
    armWatchdog();
  };
  const armWatchdog = () => {
    if (stallMs <= 0 || finished) return;
    const budget = openTools.size > 0 ? toolStallMs : stallMs;
    if (stallTimer) clearTimeout(stallTimer);
    if (budget <= 0) {
      stallTimer = void 0;
      return;
    }
    stallTimer = setTimeout(() => {
      void onStall();
    }, budget);
    stallTimer.unref?.();
  };
  const onDelta = ({ update }) => {
    counts[update.type] = (counts[update.type] ?? 0) + 1;
    switch (update.type) {
      case "text-delta":
        push({ type: "text-delta", text: update.text });
        break;
      case "thinking-delta":
        push({ type: "reasoning-delta", text: update.text });
        break;
      case "thinking-completed":
        push({ type: "reasoning-complete", durationMs: update.thinkingDurationMs });
        break;
      case "summary-started":
      case "summary":
      case "summary-completed":
        push({ type: "compaction" });
        break;
      case "partial-tool-call":
        push({
          type: "tool-input-partial",
          id: String(update.callId),
          name: toolDisplayName(update.toolCall),
          input: update.toolCall?.args ?? {}
        });
        break;
      case "tool-call-started":
        openTools.set(String(update.callId), toolDisplayName(update.toolCall));
        push({
          type: "tool-call",
          id: String(update.callId),
          name: toolDisplayName(update.toolCall),
          input: update.toolCall?.args ?? {}
        });
        break;
      case "tool-call-completed": {
        openTools.delete(String(update.callId));
        const tool = update.toolCall ?? {};
        const result = tool.result;
        const mcpError = tool.type === "mcp" && result?.value?.isError === true;
        push({
          type: "tool-result",
          id: String(update.callId),
          name: toolDisplayName(tool),
          result: result ?? null,
          isError: result?.status === "error" || mcpError
        });
        break;
      }
      case "turn-ended":
        openTools.clear();
        if (update.usage) {
          const summed = addUsage(options.usageBase, update.usage);
          if (summed) push({ type: "usage", usage: summed });
        }
        break;
    }
    armWatchdog();
  };
  const runHolder = {};
  const onAbort = () => {
    if (stallTimer) clearTimeout(stallTimer);
    stallTimer = void 0;
    void Promise.resolve(runHolder.run?.cancel()).catch(() => {
    });
  };
  options.abortSignal?.addEventListener("abort", onAbort);
  let runGen = 0;
  const startRun = (force) => {
    const gen = ++runGen;
    void sendWithRecovery(
      agent,
      message,
      {
        mode: options.mode,
        onDelta,
        ...options.idempotencyKey ? { idempotencyKey: options.idempotencyKey } : {},
        ...force ? { local: { force: true } } : {}
      },
      debug
    ).then(async (run) => {
      runHolder.run = run;
      if (options.abortSignal?.aborted) void Promise.resolve(run.cancel()).catch(() => {
      });
      const result = await run.wait();
      if (debug) {
        pluginLog("debug", "turn finished", {
          updates: counts,
          status: result.status,
          resultLen: (result.result ?? "").length,
          terminalError: result.error ?? null
        });
      }
      if (gen !== runGen || finished) return;
      if (result.status !== "finished") {
        if (result.status !== "cancelled" || !options.abortSignal?.aborted) {
          throw new CursorRunError(result, counts);
        }
      }
      push({ type: "finish", ...result.status === "cancelled" ? {} : { text: result.result } });
    }).catch((err) => {
      if (gen !== runGen) return;
      failure = err;
      if (debug) {
        pluginLog("debug", "send failed", {
          error: err instanceof Error ? err.message : String(err)
        });
      }
    }).finally(() => {
      if (gen !== runGen) return;
      finished = true;
      if (stallTimer) clearTimeout(stallTimer);
      wake?.();
      wake = void 0;
    });
  };
  const onStall = async () => {
    if (finished) return;
    if (options.abortSignal?.aborted) return;
    const failTerminal = async (message2) => {
      try {
        await runHolder.run?.cancel();
      } catch {
      }
      failure = new Error(message2);
      finished = true;
      if (stallTimer) clearTimeout(stallTimer);
      stallTimer = void 0;
      wake?.();
      wake = void 0;
    };
    if (anyEvent) {
      const budget = openTools.size > 0 ? toolStallMs : stallMs;
      const inFlight = [...openTools.values()];
      const toolHint = inFlight.length > 0 ? `; tool${inFlight.length > 1 ? "s" : ""} ${inFlight.map((n) => `"${n}"`).join(", ")} still in flight` : "";
      const knob = openTools.size > 0 ? "OPENCODE_CURSOR_TOOL_STALL_MS" : "OPENCODE_CURSOR_STALL_MS";
      await failTerminal(
        `Cursor run stalled (no events for ${budget}ms${toolHint}). Raise ${knob} (or set 0 to disable) if this legitimately runs longer.`
      );
      return;
    }
    if (forced) {
      await failTerminal(`Cursor run stalled twice (no events for ${stallMs}ms)`);
      return;
    }
    forced = true;
    if (debug) pluginLog("debug", "stream stalled; cancelling and resending with local.force");
    try {
      await runHolder.run?.cancel();
    } catch {
    }
    openTools.clear();
    armWatchdog();
    startRun(true);
  };
  armWatchdog();
  startRun(false);
  try {
    while (true) {
      if (queue.length > 0) {
        yield queue.shift();
        continue;
      }
      if (finished) break;
      await new Promise((resolve) => {
        wake = resolve;
      });
    }
    while (queue.length > 0) yield queue.shift();
    if (failure) throw failure;
  } finally {
    if (stallTimer) clearTimeout(stallTimer);
    options.abortSignal?.removeEventListener("abort", onAbort);
  }
}
var sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
var RETRY_BACKOFF_MS = [500, 1500];
async function sendWithRecovery(agent, message, sendOptions, debug) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await agent.send(message, sendOptions);
    } catch (err) {
      const classified = classifyError(err);
      if (classified.kind === "agent-busy") {
        if (debug) pluginLog("debug", "agent busy; retrying send with local.force");
        return agent.send(message, { ...sendOptions, local: { force: true } });
      }
      if ((classified.kind === "rate-limit" || classified.kind === "network") && attempt < RETRY_BACKOFF_MS.length) {
        if (debug)
          pluginLog("debug", `${classified.kind}; retrying send`, {
            delayMs: RETRY_BACKOFF_MS[attempt]
          });
        await sleep(RETRY_BACKOFF_MS[attempt]);
        continue;
      }
      throw err;
    }
  }
}
async function sendAgentTurnSilently(agent, message, options) {
  if (options.abortSignal?.aborted) return void 0;
  let usage;
  for await (const event of streamAgentTurn(agent, message, options)) {
    if (event.type === "usage") usage = event.usage;
  }
  return usage;
}

// src/provider/subagent-bridge.ts
var BRIDGE_KEY = /* @__PURE__ */ Symbol.for("@oy-cli/opencode-cursor:subagent-bridge");
function getSubagentBridge() {
  return globalThis[BRIDGE_KEY];
}
function isRecord(v) {
  return typeof v === "object" && v !== null;
}
function strField(v, key) {
  return isRecord(v) && typeof v[key] === "string" ? v[key] : void 0;
}
function numField(v, key) {
  return isRecord(v) && typeof v[key] === "number" ? v[key] : void 0;
}
function formatDuration(ms) {
  if (ms < 1e3) return `${ms}ms`;
  if (ms < 6e4) return `${(ms / 1e3).toFixed(1)}s`;
  const minutes = Math.floor(ms / 6e4);
  const seconds = Math.floor(ms % 6e4 / 1e3);
  return `${minutes}m ${seconds}s`;
}
function activityLine(value) {
  const durationMs = numField(value, "durationMs");
  const steps = isRecord(value) && Array.isArray(value["conversationSteps"]) ? value["conversationSteps"].length : void 0;
  const bits = [];
  if (steps && steps > 0) bits.push(`${steps} step${steps === 1 ? "" : "s"}`);
  if (typeof durationMs === "number") bits.push(`in ${formatDuration(durationMs)}`);
  return bits.length > 0 ? `_Subagent ran ${bits.join(" ")}._` : void 0;
}
var UNSPECIFIED_KIND = "unspecified";
function subagentLabel(args) {
  const sub = isRecord(args) ? args["subagentType"] : void 0;
  const name = strField(sub, "name");
  if (name) return name;
  const kind = strField(sub, "kind");
  if (kind && kind !== UNSPECIFIED_KIND) return kind;
  return "general";
}
function buildTranscript(value) {
  const parts = [];
  const suffix = strField(value, "resultSuffix");
  if (suffix) parts.push(suffix);
  if (isRecord(value) && Array.isArray(value["conversationSteps"])) {
    const steps = value["conversationSteps"];
    const rendered = steps.flatMap((s) => {
      const text = strField(s, "text") ?? strField(s, "content");
      return text ? [text] : [];
    }).join("\n\n");
    if (rendered) parts.push(rendered);
  }
  const activity = activityLine(value);
  if (activity) parts.push(activity);
  const body = parts.join("\n\n").trim();
  return body.length > 0 ? body : void 0;
}
async function linkSubagentSession(opts) {
  const bridge = getSubagentBridge();
  if (!bridge) return void 0;
  const { client, directory } = bridge;
  const query = directory ? { directory } : void 0;
  try {
    const description = strField(opts.args, "description") ?? "Subagent task";
    const agent = subagentLabel(opts.args);
    const created = await client.session.create({
      body: {
        parentID: opts.parentSessionID,
        title: `${description} (@${agent} subagent)`
      },
      ...query ? { query } : {}
    });
    const childId = created?.data?.id;
    if (!childId) return void 0;
    const prompt = strField(opts.args, "prompt");
    if (prompt) {
      await client.session.prompt({
        path: { id: childId },
        ...query ? { query } : {},
        body: { noReply: true, parts: [{ type: "text", text: prompt }] }
      });
    }
    const value = isRecord(opts.result) && opts.result["status"] === "success" ? opts.result["value"] : void 0;
    const transcript = buildTranscript(value);
    if (transcript) {
      await client.session.prompt({
        path: { id: childId },
        ...query ? { query } : {},
        body: { noReply: true, parts: [{ type: "text", text: transcript }] }
      });
    }
    return childId;
  } catch {
    return void 0;
  }
}

// src/provider/stream-map.ts
var TASK_TOOL_NAME = "task";
function injectSubagentSessionId(parts, sessionId) {
  for (const part of parts) {
    if (part.type !== "tool-result" || part.toolName !== "task") continue;
    const folded = part.result;
    if (!isRecord2(folded)) continue;
    const metadata = isRecord2(folded["metadata"]) ? folded["metadata"] : {};
    metadata["sessionId"] = sessionId;
    folded["metadata"] = metadata;
  }
}
function effectiveToolDisplay(configured, tools) {
  if (!Array.isArray(tools) || tools.length === 0) return "reasoning";
  return configured ?? "blocks";
}
var FINISH_STOP = {
  unified: "stop",
  raw: void 0
};
var FINISH_ERROR = {
  unified: "error",
  raw: void 0
};
function safeJsonString(input) {
  try {
    return typeof input === "string" ? input : JSON.stringify(input ?? {});
  } catch {
    return "{}";
  }
}
function blockToolName(name) {
  return `cursor_${name.replace(/[^A-Za-z0-9_-]/g, "_")}`;
}
function nativeToolCall(id, toolName, input) {
  return {
    type: "tool-call",
    toolCallId: id,
    toolName,
    input: safeJsonString(input),
    providerExecuted: true,
    dynamic: true
  };
}
function nativeToolResult(id, toolName, result, isError) {
  return {
    type: "tool-result",
    toolCallId: id,
    toolName,
    result: result ?? null,
    isError,
    providerExecuted: true,
    dynamic: true
  };
}
function toolCallObj(id, name, input) {
  return nativeToolCall(id, blockToolName(name), input);
}
function toolResultObj(id, name, result, isError) {
  return nativeToolResult(id, blockToolName(name), result, isError);
}
var EDIT_TOOL_NAME = "edit";
var CREATE_PLAN_TOOL_NAME = "createPlan";
function isCreatePlanTool(name) {
  return name === CREATE_PLAN_TOOL_NAME;
}
function createPlanContent(input) {
  return strField2(input, "plan");
}
function isRecord2(v) {
  return typeof v === "object" && v !== null;
}
function strField2(v, key) {
  return isRecord2(v) && typeof v[key] === "string" ? v[key] : void 0;
}
function numField2(v, key) {
  return isRecord2(v) && typeof v[key] === "number" ? v[key] : void 0;
}
function successValue(result) {
  return isRecord2(result) && result["status"] === "success" ? result["value"] : void 0;
}
function subagentTypeField(args) {
  const sub = isRecord2(args) ? args["subagentType"] : void 0;
  const name = strField2(sub, "name");
  const kind = strField2(sub, "kind");
  const subagent = name ?? (kind && kind !== "unspecified" ? kind : void 0);
  return subagent ? { subagent_type: subagent } : {};
}
function flattenMcpContent(value) {
  if (!isRecord2(value) || !Array.isArray(value["content"])) return null;
  const parts = value["content"].flatMap((item) => {
    const text = isRecord2(item) ? strField2(item["text"], "text") : void 0;
    if (text !== void 0) return [text];
    if (isRecord2(item) && isRecord2(item["image"])) return ["[image]"];
    return [];
  });
  return parts.join("\n");
}
function mcpFold(result) {
  const text = flattenMcpContent(successValue(result));
  if (text === null) return null;
  return { title: "", metadata: {}, output: text };
}
function mcpInputArgs(args) {
  return isRecord2(args) ? args["args"] : void 0;
}
function webSearchProvider(args) {
  const id = (strField2(args, "providerIdentifier") ?? "").toLowerCase();
  if (id.includes("exa")) return "exa";
  if (id.includes("parallel")) return "parallel";
  return void 0;
}
function isWebSearchName(name) {
  return /web[_-]?search/i.test(name);
}
var WEBSEARCH_ADAPTER = {
  tool: "websearch",
  input: (args) => {
    const query = strField2(mcpInputArgs(args), "query");
    return query !== void 0 ? { query } : {};
  },
  result: (value, args) => {
    const provider = webSearchProvider(args);
    return {
      title: "",
      metadata: provider ? { provider } : {},
      output: flattenMcpContent(value) ?? ""
    };
  }
};
function resolveAdapter(name, _input) {
  const exact = NATIVE_ADAPTERS[name];
  if (exact) return exact;
  if (isWebSearchName(name)) return WEBSEARCH_ADAPTER;
  return void 0;
}
function mapTodoStatus(status) {
  if (status === "inProgress") return "in_progress";
  return typeof status === "string" ? status : "pending";
}
function mapTodos(args) {
  const todos = isRecord2(args) && Array.isArray(args["todos"]) ? args["todos"] : [];
  return todos.flatMap(
    (t) => isRecord2(t) && typeof t["content"] === "string" ? [
      {
        content: t["content"],
        status: mapTodoStatus(t["status"])
      }
    ] : []
  );
}
var NATIVE_ADAPTERS = {
  // Cursor `shell` → opencode `bash` (console renderer).
  shell: {
    tool: "bash",
    input: (args) => ({ command: strField2(args, "command") ?? "" }),
    result: (value, args) => {
      if (!isRecord2(value)) return null;
      const command = strField2(args, "command") ?? "";
      const stdout = strField2(value, "stdout") ?? "";
      const stderr = strField2(value, "stderr") ?? "";
      const exit = numField2(value, "exitCode");
      const body = [stdout, stderr].filter((s) => s.length > 0).join("\n");
      const output = exit !== void 0 && exit !== 0 ? `${body}${body ? "\n" : ""}(exit ${exit})` : body;
      return {
        title: command,
        metadata: { command, output, exit: exit ?? 0 },
        output
      };
    }
  },
  // Cursor `read` → opencode `read`.
  //
  // Cursor's read tool takes ONLY `{ path }` — there is no `offset`/`limit`/
  // line-range arg (see `@cursor/sdk` `ReadArgsSchema`, a `{ path: string }`
  // "strip" object). The model therefore cannot request a sub-range, so two
  // same-path reads in a transcript are RE-READS of the same content, never
  // "different sections". The only per-call detail available is in the result
  // `value` (`content`, `totalLines`, `fileSize`); we derive the lines-read
  // count from `content` and surface `linesReturned/totalLines` in the title
  // so a reader can see how much was read and spot redundant re-reads.
  read: {
    tool: "read",
    input: (args) => ({ filePath: strField2(args, "path") ?? "" }),
    result: (value, args) => {
      const content = strField2(value, "content");
      if (content === void 0) return null;
      const filePath = strField2(args, "path") ?? "";
      const totalLines = numField2(value, "totalLines");
      const fileSize = numField2(value, "fileSize");
      const linesReturned = content.split("\n").length;
      const lineLabel = totalLines !== void 0 ? `${linesReturned}/${totalLines} lines` : `${linesReturned} lines`;
      return {
        title: `${filePath} (${lineLabel})`,
        metadata: {
          preview: content.split("\n").slice(0, 20).join("\n"),
          loaded: [],
          linesReturned,
          ...totalLines !== void 0 ? { totalLines } : {},
          ...fileSize !== void 0 ? { fileSize } : {}
        },
        output: content
      };
    }
  },
  // Cursor `write` → opencode `write` (renders input.content as the new file).
  write: {
    tool: "write",
    input: (args) => ({
      filePath: strField2(args, "path") ?? "",
      content: strField2(args, "fileText") ?? ""
    }),
    result: (value, args) => {
      const filePath = strField2(args, "path") ?? "";
      const lines = numField2(value, "linesCreated");
      const output = lines !== void 0 ? `Wrote ${lines} line${lines === 1 ? "" : "s"}.` : "Wrote file successfully.";
      return {
        title: filePath,
        metadata: { diagnostics: {}, filepath: filePath, exists: false },
        output
      };
    }
  },
  // Cursor `glob` → opencode `glob`.
  glob: {
    tool: "glob",
    input: (args) => {
      const pattern = strField2(args, "globPattern") ?? "";
      const dir = strField2(args, "targetDirectory");
      return dir ? { pattern, path: dir } : { pattern };
    },
    result: (value, args) => {
      if (!isRecord2(value) || !Array.isArray(value["files"])) return null;
      const files = value["files"].filter(
        (f) => typeof f === "string"
      );
      const truncated = value["clientTruncated"] === true || value["ripgrepTruncated"] === true;
      const pattern = strField2(args, "globPattern") ?? "";
      const dir = strField2(args, "targetDirectory");
      return {
        title: dir ? `${pattern} in ${dir}` : pattern,
        metadata: { count: files.length, truncated },
        output: files.length > 0 ? files.join("\n") : "No files found"
      };
    }
  },
  // Cursor `grep` → opencode `grep` (flatten matches into ripgrep-style text).
  grep: {
    tool: "grep",
    input: (args) => {
      const out = {
        pattern: strField2(args, "pattern") ?? ""
      };
      const p = strField2(args, "path");
      if (p) out["path"] = p;
      const g = strField2(args, "glob");
      if (g) out["include"] = g;
      return out;
    },
    result: (value, args) => {
      if (!isRecord2(value)) return null;
      const unions = [];
      const ws = value["workspaceResults"];
      if (isRecord2(ws)) unions.push(...Object.values(ws));
      if (value["activeEditorResult"] !== void 0)
        unions.push(value["activeEditorResult"]);
      const lines = [];
      let total = 0;
      let current = "";
      for (const u of unions) {
        if (!isRecord2(u)) continue;
        const output = u["output"];
        if (u["type"] === "content" && isRecord2(output) && Array.isArray(output["matches"])) {
          for (const m of output["matches"]) {
            if (!isRecord2(m)) continue;
            const file = strField2(m, "file") ?? "";
            const line = numField2(m, "lineNumber");
            const text = strField2(m, "line") ?? "";
            if (current !== file) {
              if (current) lines.push("");
              current = file;
              lines.push(`${file}:`);
            }
            lines.push(
              line !== void 0 ? `  Line ${line}: ${text}` : `  ${text}`
            );
            total++;
          }
        } else if (u["type"] === "files" && isRecord2(output) && Array.isArray(output["files"])) {
          for (const f of output["files"]) {
            if (typeof f === "string") {
              lines.push(f);
              total++;
            }
          }
        }
      }
      const pattern = strField2(args, "pattern") ?? "";
      const glob = strField2(args, "glob");
      const path = strField2(args, "path");
      let title = pattern;
      if (glob) title += ` (${glob})`;
      if (path) title += ` in ${path}`;
      return {
        title,
        metadata: { matches: total, truncated: false },
        output: total > 0 ? [
          `Found ${total} match${total === 1 ? "" : "es"}`,
          "",
          ...lines
        ].join("\n") : "No matches found"
      };
    }
  },
  // Cursor `semSearch` has no opencode counterpart — format-only: query title
  // + results body instead of the raw `{results}` JSON.
  semSearch: {
    input: (args) => {
      const out = { query: strField2(args, "query") ?? "" };
      const dirs = isRecord2(args) && Array.isArray(args["targetDirectories"]) ? args["targetDirectories"] : void 0;
      if (dirs && dirs.length > 0) out["targetDirectories"] = dirs;
      return out;
    },
    result: (value, args) => {
      const query = strField2(args, "query") ?? "";
      const results = strField2(value, "results") ?? "";
      const count = results ? results.split("\n").filter((l) => l.trim().length > 0).length : 0;
      return {
        title: query,
        metadata: { matches: count, truncated: false },
        output: results || "No results"
      };
    }
  },
  // Cursor `ls` → opencode `list` (flatten the directory tree into paths).
  ls: {
    tool: "list",
    input: (args) => ({ path: strField2(args, "path") ?? "" }),
    result: (value) => {
      if (!isRecord2(value)) return null;
      const root = value["directoryTreeRoot"];
      if (!isRecord2(root)) return null;
      const out = [];
      const walk = (node) => {
        const base = strField2(node, "absPath") ?? "";
        const files = Array.isArray(node["childrenFiles"]) ? node["childrenFiles"] : [];
        for (const f of files) {
          const name = strField2(f, "name");
          if (name) out.push(`${base}/${name}`);
        }
        const dirs = Array.isArray(node["childrenDirs"]) ? node["childrenDirs"] : [];
        for (const d of dirs) {
          if (!isRecord2(d)) continue;
          out.push(`${strField2(d, "absPath") ?? ""}/`);
          walk(d);
        }
      };
      walk(root);
      return {
        title: strField2(root, "absPath") ?? "",
        metadata: {},
        output: out.length > 0 ? out.join("\n") : "(empty)"
      };
    }
  },
  // Cursor `updateTodos` → opencode `todowrite` (todo checklist renderer).
  updateTodos: {
    tool: "todowrite",
    input: (args) => ({ todos: mapTodos(args) }),
    result: (_value, args) => {
      const todos = mapTodos(args);
      const done = todos.filter((t) => t.status === "completed").length;
      return {
        title: `${done}/${todos.length}`,
        metadata: { todos },
        output: `Updated ${todos.length} todo${todos.length === 1 ? "" : "s"}.`
      };
    }
  },
  // Cursor `task` (subagent) → opencode `task` (agent card: name + description).
  // Made navigable in the stream layer: when a parent session + the plugin
  // bridge are present, a real opencode child session is created and its id is
  // stamped onto this result's `metadata.sessionId` (see cursorEventsToStream's
  // tool-result path). Without that link it degrades to a non-navigable card.
  task: {
    tool: "task",
    input: (args) => ({
      description: strField2(args, "description") ?? "",
      ...subagentTypeField(args)
    }),
    result: (value, args) => {
      const description = strField2(args, "description") ?? "";
      const suffix = strField2(value, "resultSuffix");
      const background = isRecord2(value) && value["isBackground"] === true;
      return {
        title: description,
        metadata: background ? { background: true } : {},
        output: suffix ?? "Subagent task completed."
      };
    }
  },
  // Cursor `readLints` has no opencode counterpart — format-only: render the
  // diagnostics as a readable list instead of dumping the nested JSON.
  readLints: {
    input: (args) => {
      const paths = isRecord2(args) && Array.isArray(args["paths"]) ? args["paths"] : [];
      return { paths };
    },
    result: (value) => {
      const files = isRecord2(value) && Array.isArray(value["fileDiagnostics"]) ? value["fileDiagnostics"] : [];
      const lines = [];
      let total = 0;
      for (const file of files) {
        if (!isRecord2(file)) continue;
        const diags = Array.isArray(file["diagnostics"]) ? file["diagnostics"] : [];
        if (diags.length === 0) continue;
        lines.push(`${strField2(file, "path") ?? ""}`);
        for (const d of diags) {
          if (!isRecord2(d)) continue;
          const severity = strField2(d, "severity") ?? "info";
          const start = isRecord2(d["range"]) ? d["range"]["start"] : void 0;
          const line = numField2(start, "line");
          const char = numField2(start, "character");
          const loc = line !== void 0 ? ` L${line + 1}${char !== void 0 ? `:${char + 1}` : ""}` : "";
          lines.push(`  ${severity}${loc}: ${strField2(d, "message") ?? ""}`);
          total++;
        }
      }
      return {
        title: total > 0 ? `${total} problem${total === 1 ? "" : "s"}` : "No problems",
        metadata: { count: total },
        output: total > 0 ? lines.join("\n") : "No problems found."
      };
    }
  },
  // Cursor `delete` has no opencode counterpart — format-only: a one-line
  // confirmation instead of `{"fileSize":N}`.
  delete: {
    input: (args) => ({ path: strField2(args, "path") ?? "" }),
    result: (value, args) => {
      const path = strField2(args, "path") ?? "";
      const size = numField2(value, "fileSize");
      return {
        title: path,
        metadata: {},
        output: size !== void 0 ? `Deleted ${path} (${size} bytes).` : `Deleted ${path}.`
      };
    }
  }
};
function editFilePath(input) {
  return isRecord2(input) && typeof input["path"] === "string" ? input["path"] : "";
}
function editDiffString(result) {
  if (!isRecord2(result) || result["status"] !== "success") return null;
  const value = result["value"];
  if (!isRecord2(value)) return null;
  const diff = value["diffString"];
  return typeof diff === "string" && diff.length > 0 ? diff : null;
}
function reconstructEditStrings(diff) {
  const oldLines = [];
  const newLines = [];
  for (const line of diff.split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++") || line.startsWith("@@") || line.startsWith("Index:") || line.startsWith("===")) {
      continue;
    }
    if (line.startsWith("-")) oldLines.push(line.slice(1));
    else if (line.startsWith("+")) newLines.push(line.slice(1));
  }
  return { oldString: oldLines.join("\n"), newString: newLines.join("\n") };
}
function editCallFields(id, filePath, diff) {
  const { oldString, newString } = reconstructEditStrings(diff);
  return {
    type: "tool-call",
    toolCallId: id,
    toolName: EDIT_TOOL_NAME,
    input: safeJsonString({ filePath, oldString, newString }),
    providerExecuted: true,
    dynamic: true
  };
}
function editResultFields(id, filePath, diff, result) {
  const value = isRecord2(result) ? result["value"] : void 0;
  const added = isRecord2(value) && typeof value["linesAdded"] === "number" ? value["linesAdded"] : void 0;
  const removed = isRecord2(value) && typeof value["linesRemoved"] === "number" ? value["linesRemoved"] : void 0;
  const counts = added !== void 0 || removed !== void 0 ? ` (+${added ?? 0}/-${removed ?? 0})` : "";
  return {
    type: "tool-result",
    toolCallId: id,
    toolName: EDIT_TOOL_NAME,
    result: {
      title: filePath,
      metadata: { diff, diagnostics: {} },
      output: `Edit applied${counts}.`
    },
    isError: false,
    providerExecuted: true,
    dynamic: true
  };
}
function newBlockToolState() {
  return {
    open: /* @__PURE__ */ new Map(),
    pendingEdits: /* @__PURE__ */ new Map(),
    dropped: /* @__PURE__ */ new Set(),
    partials: /* @__PURE__ */ new Map(),
    planText: /* @__PURE__ */ new Map()
  };
}
function resolveToolName(name, input) {
  const adapter = resolveAdapter(name, input);
  return {
    toolName: adapter?.tool ?? blockToolName(name),
    ...adapter ? { adapter } : {}
  };
}
function blockToolInputPartialParts(id, name, input, state) {
  if (process.env["OPENCODE_CURSOR_TOOL_INPUT_STREAM"] === "0") return [];
  const serialized = safeJsonString(input);
  const prev = state.partials.get(id);
  const toolName = prev?.toolName ?? resolveToolName(name, input).toolName;
  state.partials.set(id, { toolName, serialized });
  const parts = [];
  if (!prev)
    parts.push({
      type: "tool-input-start",
      id,
      toolName,
      providerExecuted: true,
      dynamic: true
    });
  const delta = prev && serialized.startsWith(prev.serialized) ? serialized.slice(prev.serialized.length) : serialized;
  if (delta) parts.push({ type: "tool-input-delta", id, delta });
  return parts;
}
function blockToolCallParts(id, name, input, state) {
  if (name === EDIT_TOOL_NAME) {
    state.pendingEdits.set(id, editFilePath(input));
    return [];
  }
  const adapter = resolveAdapter(name, input);
  if (adapter) {
    const toolName2 = adapter.tool ?? blockToolName(name);
    state.open.set(id, { toolName: toolName2, adapter, args: input });
    return [nativeToolCall(id, toolName2, adapter.input(input))];
  }
  const toolName = blockToolName(name);
  state.open.set(id, { toolName, args: input });
  return [nativeToolCall(id, toolName, input)];
}
function blockToolResultParts(id, name, result, isError, state) {
  if (state.pendingEdits.has(id)) {
    const filePath = state.pendingEdits.get(id);
    state.pendingEdits.delete(id);
    const diff = isError ? null : editDiffString(result);
    if (diff && filePath) {
      return [
        editCallFields(id, filePath, diff),
        editResultFields(id, filePath, diff, result)
      ];
    }
    return [
      toolCallObj(id, EDIT_TOOL_NAME, { path: filePath }),
      toolResultObj(id, EDIT_TOOL_NAME, result, isError)
    ];
  }
  const open = state.open.get(id);
  state.open.delete(id);
  const toolName = open?.toolName ?? blockToolName(name);
  if (open?.adapter && !isError) {
    const value = successValue(result);
    if (value !== void 0) {
      const folded = open.adapter.result(value, open.args);
      if (folded) return [nativeToolResult(id, toolName, folded, false)];
    }
  }
  if (!isError) {
    const folded = mcpFold(result);
    if (folded) return [nativeToolResult(id, toolName, folded, false)];
  }
  return [nativeToolResult(id, toolName, result, isError)];
}
function blockDanglingParts(state) {
  const parts = [];
  for (const [id, open] of state.open) {
    parts.push(nativeToolResult(id, open.toolName, DANGLING_TOOL_RESULT, true));
  }
  state.open.clear();
  for (const [id, filePath] of state.pendingEdits) {
    parts.push(toolCallObj(id, EDIT_TOOL_NAME, { path: filePath }));
    parts.push(toolResultObj(id, EDIT_TOOL_NAME, DANGLING_TOOL_RESULT, true));
  }
  state.pendingEdits.clear();
  for (const [id, partial] of state.partials) {
    parts.push({ type: "tool-input-end", id });
    let args = {};
    try {
      args = JSON.parse(partial.serialized);
    } catch {
      args = {};
    }
    parts.push(nativeToolCall(id, partial.toolName, args));
    parts.push(
      nativeToolResult(id, partial.toolName, DANGLING_TOOL_RESULT, true)
    );
  }
  state.partials.clear();
  return parts;
}
var EMPTY_USAGE = {
  inputTokens: {
    total: void 0,
    noCache: void 0,
    cacheRead: void 0,
    cacheWrite: void 0
  },
  outputTokens: { total: void 0, text: void 0, reasoning: void 0 }
};
function mapUsage(usage) {
  return {
    inputTokens: {
      total: usage.inputTokens + usage.cacheReadTokens + usage.cacheWriteTokens,
      noCache: usage.inputTokens,
      cacheRead: usage.cacheReadTokens,
      cacheWrite: usage.cacheWriteTokens
    },
    outputTokens: {
      total: usage.outputTokens,
      text: void 0,
      reasoning: usage.reasoningTokens
    }
  };
}
function formatToolCall(name, input) {
  let arg = "";
  try {
    const s = typeof input === "string" ? input : JSON.stringify(input);
    if (s && s !== "{}" && s !== '""')
      arg = ` ${s.length > 120 ? `${s.slice(0, 120)}\u2026` : s}`;
  } catch {
  }
  return `[tool] ${name}${arg}`;
}
var DANGLING_TOOL_RESULT = {
  status: "error",
  error: "Cursor run ended before this tool call completed."
};
function cursorEventsToStream(events, toolDisplay = "blocks", ctx = {}) {
  return new ReadableStream({
    async start(controller) {
      controller.enqueue({ type: "stream-start", warnings: [] });
      let textId;
      let textCount = 0;
      let reasoningId;
      let reasoningCount = 0;
      let usage;
      let streamedText = false;
      let thinkingMs = 0;
      let compactions = 0;
      const toolState = newBlockToolState();
      const closeDanglingToolCalls = () => {
        for (const part of blockDanglingParts(toolState)) {
          controller.enqueue(part);
        }
      };
      const closeReasoning = () => {
        if (reasoningId) {
          controller.enqueue({ type: "reasoning-end", id: reasoningId });
          reasoningId = void 0;
        }
      };
      const closeText = () => {
        if (textId) {
          controller.enqueue({ type: "text-end", id: textId });
          textId = void 0;
        }
      };
      const ensureText = () => {
        closeReasoning();
        if (!textId) {
          textId = `text-${textCount++}`;
          controller.enqueue({ type: "text-start", id: textId });
        }
        return textId;
      };
      const ensureReasoning = () => {
        closeText();
        if (!reasoningId) {
          reasoningId = `reasoning-${reasoningCount++}`;
          controller.enqueue({ type: "reasoning-start", id: reasoningId });
        }
        return reasoningId;
      };
      const reasoningLine = (text) => {
        controller.enqueue({
          type: "reasoning-delta",
          id: ensureReasoning(),
          delta: text
        });
      };
      try {
        for await (const event of events) {
          switch (event.type) {
            case "text-delta":
              streamedText = true;
              controller.enqueue({
                type: "text-delta",
                id: ensureText(),
                delta: event.text
              });
              break;
            case "reasoning-delta":
              reasoningLine(event.text);
              break;
            case "tool-input-partial":
              if (isCreatePlanTool(event.name)) {
                const plan = createPlanContent(event.input) ?? "";
                const prevPlan = toolState.planText.get(event.id) ?? "";
                if (plan.length > prevPlan.length) {
                  closeReasoning();
                  streamedText = true;
                  controller.enqueue({
                    type: "text-delta",
                    id: ensureText(),
                    delta: plan.slice(prevPlan.length)
                  });
                }
                toolState.planText.set(event.id, plan);
                toolState.dropped.add(event.id);
                break;
              }
              if (toolDisplay === "blocks" && event.name !== EDIT_TOOL_NAME) {
                const parts = blockToolInputPartialParts(
                  event.id,
                  event.name,
                  event.input,
                  toolState
                );
                if (parts.length > 0) {
                  closeText();
                  closeReasoning();
                }
                for (const part of parts) controller.enqueue(part);
              }
              break;
            case "tool-call":
              if (isCreatePlanTool(event.name)) {
                const plan = createPlanContent(event.input) ?? "";
                const prevPlan = toolState.planText.get(event.id) ?? "";
                if (plan.length > prevPlan.length) {
                  closeReasoning();
                  streamedText = true;
                  controller.enqueue({
                    type: "text-delta",
                    id: ensureText(),
                    delta: plan.slice(prevPlan.length)
                  });
                }
                toolState.planText.set(event.id, plan);
                toolState.dropped.add(event.id);
                break;
              }
              if (toolDisplay === "blocks") {
                if (toolState.partials.delete(event.id)) {
                  controller.enqueue({
                    type: "tool-input-end",
                    id: event.id
                  });
                }
                const parts = blockToolCallParts(
                  event.id,
                  event.name,
                  event.input,
                  toolState
                );
                if (parts.length > 0) {
                  closeText();
                  closeReasoning();
                }
                for (const part of parts) {
                  controller.enqueue(part);
                }
              } else {
                reasoningLine(`
${formatToolCall(event.name, event.input)}
`);
              }
              break;
            case "tool-result":
              if (toolState.dropped.delete(event.id)) break;
              if (toolDisplay === "blocks") {
                const taskArgs = event.name === TASK_TOOL_NAME ? toolState.open.get(event.id)?.args : void 0;
                const parts = blockToolResultParts(
                  event.id,
                  event.name,
                  event.result,
                  event.isError,
                  toolState
                );
                if (parts.length > 0) {
                  closeText();
                  closeReasoning();
                }
                if (event.name === TASK_TOOL_NAME && !event.isError && ctx.sessionID) {
                  const childId = await linkSubagentSession({
                    parentSessionID: ctx.sessionID,
                    args: taskArgs,
                    result: event.result
                  });
                  if (childId) injectSubagentSessionId(parts, childId);
                }
                for (const part of parts) {
                  controller.enqueue(part);
                }
              } else if (event.isError) {
                reasoningLine(`[tool] ${event.name} failed
`);
              }
              break;
            case "usage":
              usage = mapUsage(event.usage);
              break;
            case "reasoning-complete":
              closeReasoning();
              if (typeof event.durationMs === "number")
                thinkingMs += event.durationMs;
              break;
            case "compaction":
              compactions++;
              break;
            case "finish":
              if (!streamedText && event.text) {
                controller.enqueue({
                  type: "text-delta",
                  id: ensureText(),
                  delta: event.text
                });
              }
              break;
          }
        }
        closeDanglingToolCalls();
        closeReasoning();
        closeText();
        {
          const cursorMeta = {};
          if (thinkingMs > 0) cursorMeta["thinkingDurationMs"] = thinkingMs;
          if (compactions > 0) cursorMeta["compactions"] = compactions;
          controller.enqueue({
            type: "finish",
            usage: usage ?? EMPTY_USAGE,
            finishReason: FINISH_STOP,
            ...Object.keys(cursorMeta).length > 0 ? { providerMetadata: { cursor: cursorMeta } } : {}
          });
        }
        controller.close();
      } catch (err) {
        controller.enqueue({ type: "error", error: err });
        closeDanglingToolCalls();
        closeReasoning();
        closeText();
        {
          const cursorMeta = {};
          if (thinkingMs > 0) cursorMeta["thinkingDurationMs"] = thinkingMs;
          if (compactions > 0) cursorMeta["compactions"] = compactions;
          controller.enqueue({
            type: "finish",
            usage: usage ?? EMPTY_USAGE,
            finishReason: FINISH_ERROR,
            ...Object.keys(cursorMeta).length > 0 ? { providerMetadata: { cursor: cursorMeta } } : {}
          });
        }
        controller.close();
      }
    }
  });
}
async function cursorEventsToContent(events, toolDisplay = "blocks") {
  const content = [];
  const toolParts = [];
  const toolState = newBlockToolState();
  let text = "";
  let reasoning = "";
  let usage = EMPTY_USAGE;
  let finishReason = FINISH_STOP;
  try {
    for await (const event of events) {
      switch (event.type) {
        case "text-delta":
          text += event.text;
          break;
        case "reasoning-delta":
          reasoning += event.text;
          break;
        case "tool-input-partial":
          break;
        case "tool-call":
          if (isCreatePlanTool(event.name)) {
            const plan = createPlanContent(event.input);
            if (plan) text += plan;
            toolState.dropped.add(event.id);
            break;
          }
          if (toolDisplay === "blocks") {
            for (const part of blockToolCallParts(
              event.id,
              event.name,
              event.input,
              toolState
            )) {
              toolParts.push(part);
            }
          } else {
            reasoning += `
${formatToolCall(event.name, event.input)}
`;
          }
          break;
        case "tool-result":
          if (toolState.dropped.delete(event.id)) break;
          if (toolDisplay === "blocks") {
            for (const part of blockToolResultParts(
              event.id,
              event.name,
              event.result,
              event.isError,
              toolState
            )) {
              toolParts.push(part);
            }
          } else if (event.isError) {
            reasoning += `[tool] ${event.name} failed
`;
          }
          break;
        case "usage":
          usage = mapUsage(event.usage);
          break;
        case "reasoning-complete":
          break;
        case "compaction":
          break;
        case "finish":
          if (!text && event.text) text = event.text;
          break;
      }
    }
  } catch {
    finishReason = FINISH_ERROR;
  }
  for (const part of blockDanglingParts(toolState)) {
    toolParts.push(part);
  }
  if (reasoning) content.push({ type: "reasoning", text: reasoning });
  content.push(...toolParts);
  if (text) content.push({ type: "text", text });
  return { content, finishReason, usage };
}

// src/provider/controls.ts
function buildModelSelection(modelId, params) {
  const paramList = Object.entries(params ?? {}).map(([id, value]) => ({ id, value }));
  return paramList.length > 0 ? { id: modelId, params: paramList } : { id: modelId };
}
function isRecord3(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function isMode(value) {
  return value === "agent" || value === "plan";
}
function resolveControls(modelId, staticControls, providerOptions) {
  const po = providerOptions ?? {};
  const mode = isMode(po["mode"]) ? po["mode"] : staticControls.mode;
  const params = {
    ...staticControls.defaults ?? {},
    ...staticControls.params ?? {}
  };
  if (isRecord3(po["params"])) {
    for (const [key, value] of Object.entries(po["params"])) {
      if (value != null) params[key] = String(value);
    }
  }
  if (typeof po["thinking"] === "string" && params["thinking"] === void 0) {
    params["thinking"] = po["thinking"];
  }
  return { mode, modelSelection: buildModelSelection(modelId, params) };
}

// src/provider/session-store.ts
import { mkdirSync as mkdirSync2, readFileSync as readFileSync2, rmSync as rmSync2, writeFileSync as writeFileSync2 } from "fs";
import { homedir, tmpdir } from "os";
import { join as join2 } from "path";
var ENTRY_TTL_MS = 7 * 24 * 60 * 60 * 1e3;
var MAX_ENTRIES = 200;
function storeDir() {
  const base = process.env.XDG_CACHE_HOME?.trim() || (homedir() ? join2(homedir(), ".cache") : tmpdir());
  return join2(base, "opencode-cursor");
}
function storeFile() {
  return join2(storeDir(), "session-pool.json");
}
function isStoredRecord(value) {
  if (typeof value !== "object" || value === null) return false;
  const v = value;
  return typeof v["agentId"] === "string" && typeof v["systemHash"] === "string" && Array.isArray(v["userHashes"]) && v["userHashes"].every((h) => typeof h === "string") && typeof v["updatedAt"] === "number";
}
function loadSessionRecords(now = Date.now()) {
  const out = /* @__PURE__ */ new Map();
  try {
    const parsed = JSON.parse(
      readFileSync2(storeFile(), "utf8")
    );
    if (typeof parsed?.sessions !== "object" || parsed.sessions === null)
      return out;
    for (const [key, value] of Object.entries(parsed.sessions)) {
      if (!isStoredRecord(value)) continue;
      if (now - value.updatedAt > ENTRY_TTL_MS) continue;
      out.set(key, value);
    }
  } catch {
  }
  return out;
}
function saveSessionRecords(records, now = Date.now()) {
  try {
    const live = [...records.entries()].filter(([, r]) => now - r.updatedAt <= ENTRY_TTL_MS).sort(([, a], [, b]) => b.updatedAt - a.updatedAt).slice(0, MAX_ENTRIES);
    mkdirSync2(storeDir(), { recursive: true });
    const envelope = { sessions: Object.fromEntries(live) };
    writeFileSync2(storeFile(), JSON.stringify(envelope), "utf8");
  } catch {
  }
}

// src/provider/session-pool.ts
var pool = /* @__PURE__ */ new Map();
var hydrated = false;
function hydrate() {
  if (hydrated) return;
  hydrated = true;
  for (const [key, record] of loadSessionRecords()) {
    if (!pool.has(key)) pool.set(key, record);
  }
}
function getSessionRecord(sessionID) {
  hydrate();
  return pool.get(sessionID);
}
function dropSessionRecord(sessionID) {
  hydrate();
  if (pool.delete(sessionID)) saveSessionRecords(pool);
}
var sessionLocks = /* @__PURE__ */ new Map();
function withSessionLock(sessionID, fn) {
  if (!sessionID) return fn();
  const prior = sessionLocks.get(sessionID) ?? Promise.resolve();
  const run = prior.then(fn, fn);
  const guarded = run.catch(() => {
  });
  sessionLocks.set(sessionID, guarded);
  void guarded.finally(() => {
    if (sessionLocks.get(sessionID) === guarded) sessionLocks.delete(sessionID);
  });
  return run;
}
async function acquireAgent(params) {
  const backend = loadAgentBackend(params.logger);
  const createOptions = {
    apiKey: params.apiKey,
    model: params.modelSelection,
    mode: params.mode,
    local: {
      cwd: params.cwd,
      ...params.settingSources ? { settingSources: params.settingSources } : {},
      ...params.sandbox !== void 0 ? { sandboxOptions: { enabled: params.sandbox } } : {},
      ...params.autoReview !== void 0 ? { autoReview: params.autoReview } : {}
    },
    ...params.mcpServers ? { mcpServers: params.mcpServers } : {},
    ...params.agents ? { agents: params.agents } : {},
    ...params.name ? { name: params.name } : {}
  };
  let agent;
  let resumed = false;
  if (params.resumeAgentId) {
    try {
      agent = await backend.resumeAgent(params.resumeAgentId, createOptions);
      resumed = true;
    } catch {
    }
  }
  if (!agent) {
    agent = await backend.createAgent(createOptions);
  }
  const pooling = params.poolKey !== void 0;
  if (pooling && params.record) {
    hydrate();
    pool.set(params.poolKey, {
      agentId: agent.agentId,
      systemHash: params.record.systemHash,
      userHashes: params.record.userHashes,
      ...params.record.mcpHash !== void 0 ? { mcpHash: params.record.mcpHash } : {},
      updatedAt: Date.now()
    });
    saveSessionRecords(pool);
  }
  const release = () => {
    if (!pooling) {
      try {
        agent.close();
      } catch {
      }
    }
  };
  const discard = () => {
    try {
      agent.close();
    } catch {
    }
    if (params.poolKey !== void 0) {
      hydrate();
      if (pool.get(params.poolKey)?.agentId === agent.agentId) {
        pool.delete(params.poolKey);
        saveSessionRecords(pool);
      }
    }
  };
  return { agent, resumed, release, discard };
}

// src/provider/transcript-fingerprint.ts
import { createHash as createHash2 } from "crypto";
function sha(input) {
  return createHash2("sha256").update(input).digest("hex");
}
function mcpServersFingerprint(servers) {
  if (!servers) return "";
  const keys = Object.keys(servers).sort();
  if (keys.length === 0) return "";
  return sha(JSON.stringify(keys.map((k) => [k, servers[k]])));
}
function userMessageKey(message) {
  const parts = [];
  for (const part of message.content) {
    if (part.type === "text") parts.push(`t:${part.text}`);
    else if (part.type === "file") parts.push(`f:${part.mediaType}`);
  }
  return parts.join("\n");
}
function fingerprint(prompt) {
  const systemParts = [];
  const userHashes = [];
  for (const message of prompt) {
    if (message.role === "system") systemParts.push(message.content);
    else if (message.role === "user")
      userHashes.push(sha(userMessageKey(message)));
  }
  return { systemHash: sha(systemParts.join("\n")), userHashes };
}
function isStrictPrefix(prefix, full) {
  if (prefix.length >= full.length) return false;
  for (let i = 0; i < prefix.length; i++) {
    if (prefix[i] !== full[i]) return false;
  }
  return true;
}
function classifyTurn(prev, prompt) {
  const fp = fingerprint(prompt);
  if (!prev) return { kind: "new", fingerprint: fp };
  if (prev.systemHash !== fp.systemHash)
    return { kind: "side-call", fingerprint: fp };
  const lastIsUser = prompt[prompt.length - 1]?.role === "user";
  const newUserCount = fp.userHashes.length - prev.userHashes.length;
  const isPrefix = isStrictPrefix(prev.userHashes, fp.userHashes);
  if (lastIsUser && isPrefix && newUserCount === 1) {
    return { kind: "continuation", fingerprint: fp, newUserCount: 1 };
  }
  const tailContiguous = newUserCount >= 2 && prompt.length >= newUserCount && prompt.slice(prompt.length - newUserCount).every((m) => m.role === "user");
  if (lastIsUser && isPrefix && tailContiguous) {
    return { kind: "continuation-multi", fingerprint: fp, newUserCount };
  }
  return { kind: "divergence", fingerprint: fp };
}
function sendIdempotencyKey(sessionID, record, messageText) {
  return createHash2("sha256").update(`${sessionID ?? "ephemeral"}|${record?.userHashes.join(",") ?? ""}|${messageText}`).digest("hex").slice(0, 32);
}

// src/provider/language-model.ts
var CursorLanguageModel = class {
  constructor(modelId, config) {
    this.config = config;
    this.modelId = modelId;
    this.provider = config.providerName;
  }
  config;
  specificationVersion = "v3";
  modelId;
  provider;
  // The local Cursor agent has no attachment channel, so file parts (images,
  // directories, other media) are noted as text in message-map rather than
  // fetched or attached; no URLs are resolved natively.
  supportedUrls = {};
  /** Messages already emitted, so degradation warnings fire once, not per turn. */
  warned = /* @__PURE__ */ new Set();
  warnOnce(message) {
    if (this.warned.has(message)) return;
    this.warned.add(message);
    pluginLog("warn", message, { provider: this.provider });
  }
  requireApiKey() {
    const apiKey = resolveCursorApiKey(this.config.apiKey);
    if (!apiKey) {
      throw new LoadAPIKeyError({
        message: "Cursor API key missing. Run `opencode auth login` and choose Cursor, or set CURSOR_API_KEY."
      });
    }
    return apiKey;
  }
  async *agentRun(options) {
    const providerOptions = options.providerOptions?.[this.provider];
    const { mode, modelSelection } = resolveControls(
      this.modelId,
      {
        mode: this.config.mode,
        params: this.config.params,
        defaults: this.config.modelParamDefaults?.[this.modelId]
      },
      providerOptions
    );
    if (process.env["OPENCODE_CURSOR_DEBUG"] === "1") {
      pluginLog("debug", "model call", {
        model: this.modelId,
        selection: modelSelection
      });
    }
    const sessionID = typeof providerOptions?.["sessionID"] === "string" ? providerOptions["sessionID"] : void 0;
    const dynamicMcp = providerOptions?.["mcpServers"];
    const mcpServers = dynamicMcp ?? this.config.mcpServers;
    const mcpHash = mcpServersFingerprint(mcpServers);
    const sessionEnabled = (this.config.session ?? "auto") !== false;
    const explicitAgentId = typeof providerOptions?.["agentId"] === "string" ? providerOptions["agentId"] : void 0;
    const ephemeral = providerOptions?.["ephemeral"] === true;
    const usePool = sessionEnabled && Boolean(sessionID) && !explicitAgentId;
    const {
      acquired,
      multiTurns,
      idempotencyKey,
      systemMode,
      baseAcquire,
      record
    } = await withSessionLock(usePool ? sessionID : void 0, async () => {
      let resumeAgentId = explicitAgentId;
      let poolKey;
      let record2;
      let multiNewUserCount = 0;
      if (usePool) {
        const classification = ephemeral ? {
          kind: "side-call",
          fingerprint: fingerprint(options.prompt)
        } : classifyTurn(getSessionRecord(sessionID), options.prompt);
        switch (classification.kind) {
          case "continuation":
          case "continuation-multi": {
            const prev = getSessionRecord(sessionID);
            if (prev?.mcpHash === mcpHash) {
              resumeAgentId = prev?.agentId;
            }
            poolKey = sessionID;
            record2 = { ...classification.fingerprint, mcpHash };
            if (classification.kind === "continuation-multi") {
              multiNewUserCount = classification.newUserCount ?? 0;
            }
            break;
          }
          case "new":
          case "divergence":
            poolKey = sessionID;
            record2 = { ...classification.fingerprint, mcpHash };
            break;
          case "side-call":
            break;
        }
        if (process.env["OPENCODE_CURSOR_DEBUG"] === "1") {
          const label = classification.kind === "continuation" ? "resume" : classification.kind === "continuation-multi" ? `resume-multi:${multiNewUserCount}` : `fresh:${classification.kind}`;
          pluginLog("debug", "turn classification", { label, session: sessionID });
        }
      }
      let multiTurns2;
      if (multiNewUserCount >= 2) {
        const turns = trailingUserMessages(options.prompt, multiNewUserCount);
        if (turns.length === multiNewUserCount) {
          multiTurns2 = turns;
        } else {
          resumeAgentId = void 0;
        }
      }
      const latestUser = latestUserMessage(options.prompt);
      const idempotencyKey2 = sendIdempotencyKey(
        sessionID,
        record2,
        latestUser?.text ?? JSON.stringify(options.prompt)
      );
      const dynamicSkillsCatalogue = typeof providerOptions?.["skillsCatalogue"] === "string" ? providerOptions["skillsCatalogue"] : void 0;
      const skillsCatalogue = dynamicSkillsCatalogue ?? this.config.skillsCatalogue;
      const delivery = resolveSystemDelivery({
        mode: this.config.systemPrompt ?? "rules",
        settingSources: this.config.settingSources,
        cwd: this.config.cwd,
        systemText: extractSystemText(options.prompt),
        skillsCatalogue,
        warn: (message) => this.warnOnce(message)
      });
      const systemMode2 = delivery.mode;
      const settingSources = delivery.settingSources;
      const baseAcquire2 = {
        apiKey: this.requireApiKey(),
        modelSelection,
        mode,
        cwd: this.config.cwd,
        ...settingSources ? { settingSources } : {},
        ...this.config.sandbox !== void 0 ? { sandbox: this.config.sandbox } : {},
        ...this.config.autoReview !== void 0 ? { autoReview: this.config.autoReview } : {},
        ...mcpServers ? { mcpServers } : {},
        ...this.config.agents ? { agents: this.config.agents } : {},
        ...this.config.logger ? { logger: this.config.logger } : {},
        ...poolKey ? { name: `opencode/${sessionID.slice(-8)}` } : {},
        ...poolKey ? { poolKey } : {},
        ...record2 ? { record: record2 } : {}
      };
      const acquired2 = await acquireAgent({
        ...baseAcquire2,
        ...resumeAgentId ? { resumeAgentId } : {}
      });
      return { acquired: acquired2, multiTurns: multiTurns2, idempotencyKey: idempotencyKey2, systemMode: systemMode2, baseAcquire: baseAcquire2, record: record2 };
    });
    let yielded = false;
    let releasedOriginal = false;
    try {
      if (acquired.resumed && multiTurns) {
        let delivered = false;
        try {
          let aborted = false;
          let replayUsage;
          for (let i = 0; i < multiTurns.length - 1; i++) {
            if (options.abortSignal?.aborted) {
              aborted = true;
              break;
            }
            replayUsage = addUsage(
              replayUsage,
              await sendAgentTurnSilently(acquired.agent, multiTurns[i], {
                mode,
                abortSignal: options.abortSignal,
                idempotencyKey: sendIdempotencyKey(
                  sessionID,
                  { userHashes: [...record?.userHashes ?? [], String(i)] },
                  multiTurns[i].text
                )
              })
            );
          }
          if (!aborted && !options.abortSignal?.aborted) {
            for await (const event of streamAgentTurn(
              acquired.agent,
              multiTurns[multiTurns.length - 1],
              {
                mode,
                abortSignal: options.abortSignal,
                // Key intentionally diverges from the single-turn key: the multi-replay and single-turn branches are mutually exclusive.
                idempotencyKey: sendIdempotencyKey(
                  sessionID,
                  {
                    userHashes: [
                      ...record?.userHashes ?? [],
                      String(multiTurns.length - 1)
                    ]
                  },
                  multiTurns[multiTurns.length - 1].text
                ),
                ...replayUsage ? { usageBase: replayUsage } : {}
              }
            )) {
              yielded = true;
              yield event;
            }
            delivered = true;
          }
        } finally {
          if (!delivered) {
            if (sessionID) dropSessionRecord(sessionID);
            acquired.discard();
            releasedOriginal = true;
          }
        }
      } else {
        const message = acquired.resumed ? latestUserMessage(options.prompt) ?? promptToCursorMessage(options.prompt, systemMode) : promptToCursorMessage(options.prompt, systemMode);
        try {
          for await (const event of streamAgentTurn(acquired.agent, message, {
            mode,
            abortSignal: options.abortSignal,
            idempotencyKey
          })) {
            yielded = true;
            yield event;
          }
          if (options.abortSignal?.aborted) {
            acquired.discard();
            releasedOriginal = true;
          }
        } catch (err) {
          const classified = classifyError(err);
          if (classified.kind === "auth" || classified.kind === "config") {
            throw classified.helpUrl ? new Error(
              `${classified.message} (see ${classified.helpUrl})`,
              { cause: err }
            ) : err;
          }
          const replayable = classified.kind === "agent-not-found" || classified.kind === "rate-limit" || classified.kind === "network" || classified.kind === "unknown";
          if (replayable && !yielded && !options.abortSignal?.aborted) {
            if (process.env["OPENCODE_CURSOR_DEBUG"] === "1") {
              pluginLog(
                "debug",
                `${acquired.resumed ? "resumed" : "fresh"} turn failed before emitting; retrying with a fresh agent`
              );
            }
            acquired.discard();
            releasedOriginal = true;
            let retry;
            try {
              retry = await withSessionLock(
                usePool ? sessionID : void 0,
                () => acquireAgent({ ...baseAcquire })
              );
            } catch (retryErr) {
              if (retryErr instanceof Error && retryErr.cause === void 0) {
                retryErr.cause = err;
              } else if (process.env["OPENCODE_CURSOR_DEBUG"] === "1") {
                pluginLog("debug", "original resume failure (not attachable as cause)", {
                  error: err instanceof Error ? err.message : String(err)
                });
              }
              throw retryErr;
            }
            const replay = promptToCursorMessage(options.prompt, systemMode);
            let delivered = false;
            try {
              yield* streamAgentTurn(retry.agent, replay, {
                mode,
                abortSignal: options.abortSignal,
                idempotencyKey
              });
              delivered = !options.abortSignal?.aborted;
            } finally {
              if (delivered) retry.release();
              else retry.discard();
            }
          } else {
            if (err instanceof CursorRunError) {
              acquired.discard();
              releasedOriginal = true;
            }
            throw err;
          }
        }
      }
    } finally {
      if (!releasedOriginal) acquired.release();
    }
  }
  async doStream(options) {
    return withProviderLogger(this.config.logger, () => {
      const po = options.providerOptions?.[this.provider];
      const sessionID = typeof po?.["sessionID"] === "string" ? po["sessionID"] : void 0;
      return {
        stream: cursorEventsToStream(
          this.agentRun(options),
          effectiveToolDisplay(this.config.toolDisplay, options.tools),
          { sessionID }
        )
      };
    });
  }
  async doGenerate(options) {
    return withProviderLogger(this.config.logger, async () => {
      const result = await cursorEventsToContent(
        this.agentRun(options),
        effectiveToolDisplay(this.config.toolDisplay, options.tools)
      );
      return { ...result, warnings: [] };
    });
  }
};

// src/provider/index.ts
function createCursor(options = {}) {
  if (options.transport) setPreferredTransport(options.transport);
  const mcpServers = options.mcpServers && Object.keys(options.mcpServers).length > 0 ? options.mcpServers : void 0;
  const config = {
    providerName: options.name ?? "cursor",
    apiKey: resolveCursorApiKey(options.apiKey),
    cwd: options.cwd ?? process.cwd(),
    mode: options.mode ?? "agent",
    ...options.params ? { params: options.params } : {},
    ...options.modelParamDefaults ? { modelParamDefaults: options.modelParamDefaults } : {},
    ...mcpServers ? { mcpServers } : {},
    ...options.settingSources ? { settingSources: options.settingSources } : {},
    ...options.sandbox !== void 0 ? { sandbox: options.sandbox } : {},
    ...options.autoReview !== void 0 ? { autoReview: options.autoReview } : {},
    ...options.agents ? { agents: options.agents } : {},
    session: options.session ?? "auto",
    toolDisplay: options.toolDisplay ?? "blocks",
    systemPrompt: options.systemPrompt ?? "rules",
    ...options.logger ? { logger: options.logger } : {},
    ...options.skillsCatalogue ? { skillsCatalogue: options.skillsCatalogue } : {}
  };
  const notImplemented = (kind, modelId) => {
    throw new NoSuchModelError({
      modelId,
      modelType: kind,
      message: `The Cursor provider does not support ${kind} models.`
    });
  };
  return {
    specificationVersion: "v3",
    languageModel: (modelId) => new CursorLanguageModel(modelId, config),
    embeddingModel: (modelId) => notImplemented("embeddingModel", modelId),
    imageModel: (modelId) => notImplemented("imageModel", modelId)
  };
}
export {
  createCursor
};
//# sourceMappingURL=index.js.map