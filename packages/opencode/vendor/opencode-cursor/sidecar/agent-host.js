// src/sidecar/agent-host.mjs
import { createInterface } from "readline";
function serializeError(err) {
  if (err instanceof Error) {
    const out = { name: err.name, message: err.message };
    for (const k of ["status", "code", "isRetryable", "helpUrl"]) {
      const v = err[k];
      if (typeof v === "number" || typeof v === "string" || typeof v === "boolean") out[k] = v;
    }
    return out;
  }
  return { name: "Error", message: String(err) };
}
function write(payload) {
  process.stdout.write(`${JSON.stringify(payload)}
`);
}
var ANSI_PATTERN = /\x1b\[[0-9;]*m/g;
var RULE_LOAD_PATTERN = /^\d{2}:\d{2}:\d{2}\.\d{3}\s+INFO\s+(LocalCursorRulesService|AgentSkillsCursorRulesService|CursorPluginsAgentSkillsService) load completed(?:\s+ctx=\S+)?\s+meta=\{([^}]*)\}\s*$/;
function parseLogMeta(raw) {
  const out = {};
  for (const part of raw.split(",")) {
    const [key, value] = part.split(":").map((s) => s.trim());
    if (!key || value === void 0) continue;
    const num = Number(value);
    if (Number.isFinite(num)) out[key] = num;
  }
  return out;
}
var originalConsoleLog = console.log.bind(console);
console.log = (...args) => {
  if (args.length === 1 && typeof args[0] === "string") {
    const match = RULE_LOAD_PATTERN.exec(args[0].replace(ANSI_PATTERN, ""));
    if (match) {
      const [, service, meta] = match;
      write({
        ev: "log",
        level: "info",
        message: `${service} load completed`,
        meta: parseLogMeta(meta ?? "")
      });
      return;
    }
  }
  originalConsoleLog(...args);
};
var sdkPromise;
function loadSdk() {
  sdkPromise ??= import(process.env.OPENCODE_CURSOR_SDK_PATH || "@cursor/sdk");
  return sdkPromise;
}
var agents = /* @__PURE__ */ new Map();
var runs = /* @__PURE__ */ new Map();
async function handleRequest(req) {
  const { id, op } = req;
  switch (op) {
    case "ping": {
      write({ id, ok: true, pid: process.pid });
      return;
    }
    case "create":
    case "resume": {
      const { Agent } = await loadSdk();
      const agent = op === "resume" ? await Agent.resume(req.agentId, req.options) : await Agent.create(req.options);
      agents.set(agent.agentId, agent);
      write({ id, ok: true, agentId: agent.agentId });
      return;
    }
    case "send": {
      const agent = agents.get(req.agentId);
      if (!agent) throw new Error(`unknown agent "${req.agentId}"`);
      const sendOptions = {
        ...req.mode ? { mode: req.mode } : {},
        ...req.force ? { local: { force: true } } : {},
        ...req.idempotencyKey ? { idempotencyKey: req.idempotencyKey } : {},
        onDelta: ({ update }) => write({ id, ev: "update", update })
      };
      const run = await agent.send(req.message, sendOptions);
      runs.set(id, run);
      write({ id, ok: true });
      try {
        const result = await run.wait();
        write({ id, ev: "result", result });
      } catch (err) {
        write({ id, ev: "error", error: serializeError(err) });
      } finally {
        runs.delete(id);
      }
      return;
    }
    case "cancel": {
      const run = runs.get(req.sendId);
      if (run) await run.cancel();
      write({ id, ok: true });
      return;
    }
    case "close": {
      const agent = agents.get(req.agentId);
      agents.delete(req.agentId);
      try {
        agent?.close();
      } catch {
      }
      write({ id, ok: true });
      return;
    }
    default:
      throw new Error(`unknown op "${op}"`);
  }
}
var rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let req;
  try {
    req = JSON.parse(line);
  } catch (err) {
    write({ id: null, ok: false, error: serializeError(err) });
    return;
  }
  handleRequest(req).catch((err) => {
    write({ id: req.id, ok: false, error: serializeError(err) });
  });
});
rl.on("close", () => {
  process.exit(0);
});
//# sourceMappingURL=agent-host.js.map