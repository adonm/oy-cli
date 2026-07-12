import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { Plugin } from "@opencode-ai/plugin/v2"

const assets = fileURLToPath(new URL("../assets", import.meta.url))
const agent = readFileSync(new URL("../assets/agents/oy.md", import.meta.url), "utf8")
const system = agent.split("---", 3)[2].trim()

export default Plugin.define({
  id: "oy",
  setup: async (ctx) => {
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
  },
})
