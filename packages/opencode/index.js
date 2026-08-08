import { readFileSync, rmSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { createCursor } from "@stablekernel/opencode-cursor"
import { applyCursorCatalog, fallbackCursorModels } from "./cursor-catalog.js"
import { startCursorBridge } from "./cursor-bridge.js"
import { safeWorkspacePath } from "./cursor-paths.js"
import { removeBundledCursorSkills, writeBundledCursorSkills } from "./cursor-skills.js"

const assets = fileURLToPath(new URL("./assets", import.meta.url))
const agent = readFileSync(new URL("./assets/agents/oy.md", import.meta.url), "utf8")
const frontmatter = agent.match(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)([\s\S]*)$/)
if (!frontmatter) throw new Error("assets/agents/oy.md must contain YAML frontmatter")
const system = frontmatter[1].trim()

export const defaultCursorStallMs = 1_200_000

export const applyCursorEnvironmentDefaults = (environment) => {
  if (environment.OPENCODE_CURSOR_STALL_MS === undefined) {
    environment.OPENCODE_CURSOR_STALL_MS = String(defaultCursorStallMs)
  }
}

export const isGeneratedCursorRule = (content) => {
  if (!content.startsWith("---")) return false
  const end = content.indexOf("\n---", 3)
  if (end === -1) return false
  return content
    .slice(0, end)
    .split(/\r?\n/)
    .includes("generated: opencode-cursor")
}

export const removeGeneratedCursorRule = (directory) => {
  try {
    const path = safeWorkspacePath(directory, ".cursor", "rules", "opencode.mdc")
    if (isGeneratedCursorRule(readFileSync(path, "utf8"))) rmSync(path)
  } catch {
    // The provider may not have written a rule in this location.
  }
}

const defaultDependencies = {
  createCursor,
  environment: process.env,
  listCursorModels: async (apiKey) => {
    const { Cursor } = await import("@cursor/sdk")
    return Cursor.models.list({ apiKey })
  },
  reportError: (operation, error) => {
    const detail = error instanceof Error ? `: ${error.message}` : ""
    console.warn(`[oy] ${operation}${detail}`)
  },
  startCursorBridge,
}

export const createPlugin = (overrides = {}) => {
  const dependencies = { ...defaultDependencies, ...overrides }
  return {
    id: "oy",
    setup: async (ctx) => {
      const directory =
        (typeof ctx.options?.cwd === "string" && ctx.options.cwd) ||
        dependencies.environment.OY_ROOT ||
        process.cwd()
      applyCursorEnvironmentDefaults(dependencies.environment)
      let cursorModels = fallbackCursorModels
      let disposed = false
      let refreshing
      let catalogReady = false
      let bridge

      await ctx.skill.transform((skills) => {
        skills.source({ type: "directory", path: `${assets}/skills` })
      })

      await ctx.agent.transform((agents) => {
        agents.update("oy", (draft) => {
          draft.name = "oy"
          draft.description =
            "Concise autonomous coding agent using the oy deterministic evidence CLI and the user's OpenCode permissions."
          draft.mode = "primary"
          draft.system = system
        })
      })

      await ctx.command.transform((commands) => {
        commands.update("oy-audit", (draft) => {
          draft.description = "Prepare deterministic evidence and audit every chunk."
          draft.agent = "oy"
          draft.template = "Load the `oy-audit` skill and execute it locally.\n\n$ARGUMENTS"
        })
        commands.update("oy-review", (draft) => {
          draft.description = "Prepare deterministic evidence and review every chunk."
          draft.agent = "oy"
          draft.template = "Load the `oy-review` skill and execute it locally.\n\n$ARGUMENTS"
        })
        commands.update("oy-enhance", (draft) => {
          draft.description = "Fix one finding from ISSUES.md or REVIEW.md."
          draft.agent = "oy"
          draft.template = "Load the `oy-enhance` skill and execute it locally.\n\n$ARGUMENTS"
        })
      })

      await ctx.integration.transform((integrations) => {
        integrations.update("cursor", (integration) => {
          integration.name = "Cursor"
        })
        integrations.method.update({
          integrationID: "cursor",
          method: { type: "key", label: "Cursor API key" },
        })
        integrations.method.update({
          integrationID: "cursor",
          method: { type: "env", names: ["CURSOR_API_KEY"] },
        })
      })

      await ctx.catalog.transform((catalog) => {
        applyCursorCatalog(catalog, directory, cursorModels, bridge)
        catalogReady = true
      })

      const refreshCursorModels = () => {
        if (refreshing) return refreshing
        refreshing = (async () => {
          const connection = await ctx.integration.connection.active("cursor")
          if (!connection || disposed) return
          const credential = await ctx.integration.connection.resolve(connection)
          const apiKey = credential?.type === "key" ? credential.key : credential?.access
          if (!apiKey || disposed) return
          const models = await dependencies.listCursorModels(apiKey)
          if (models.length === 0 || disposed) return
          cursorModels = models
          await ctx.catalog.reload()
        })()
          .catch((error) => {
            if (!disposed) dependencies.reportError("failed to refresh Cursor models", error)
          })
          .finally(() => {
            refreshing = undefined
          })
        return refreshing
      }

      bridge = await dependencies.startCursorBridge({
        directory,
        createCursor: dependencies.createCursor,
        listCursorModels: dependencies.listCursorModels,
        onModels: async (models) => {
          if (models.length === 0 || disposed) return
          cursorModels = models
          if (catalogReady) await ctx.catalog.reload()
        },
        onDirectory: (cwd) =>
          writeBundledCursorSkills(cwd, assets, (message) => dependencies.reportError(message)),
        onIdle: (cwd) => {
          removeGeneratedCursorRule(cwd)
          removeBundledCursorSkills(cwd)
        },
        reportError: dependencies.reportError,
      })
      if (catalogReady) await ctx.catalog.reload()

      const controller = new AbortController()
      const events = ctx.event?.subscribe
        ? (async () => {
            for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
              if (
                event.type === "integration.connection.updated" &&
                event.data.integrationID === "cursor"
              ) {
                void refreshCursorModels()
              }
            }
          })().catch((error) => {
            if (!disposed && error?.name !== "AbortError") {
              dependencies.reportError("Cursor connection event subscription failed", error)
            }
          })
        : Promise.resolve()
      void refreshCursorModels()

      return async () => {
        disposed = true
        controller.abort()
        await events
        await refreshing
        removeGeneratedCursorRule(directory)
        removeBundledCursorSkills(directory)
        await bridge?.close()
      }
    },
  }
}

export default createPlugin()
