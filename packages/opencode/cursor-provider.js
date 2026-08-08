export const bundledCursorSkills = [
  {
    id: "oy-audit",
    description:
      "Prepares deterministic repository evidence with the oy CLI, audits every chunk, and finalizes ISSUES.md or SARIF.",
  },
  {
    id: "oy-review",
    description:
      "Prepares deterministic workspace or target-diff evidence with the oy CLI, reviews every chunk, and finalizes REVIEW.md.",
  },
  {
    id: "oy-enhance",
    description: "Fixes one actionable finding from ISSUES.md or REVIEW.md, then verifies it.",
  },
]

export const defaultCursorAgents = {
  explore: {
    description: "Fast read-only codebase exploration and source-backed research.",
    prompt:
      "Explore the requested scope thoroughly with read-only tools. Do not edit files. Return concise source-backed findings with paths and symbols.",
    model: "inherit",
  },
  general: {
    description: "General-purpose execution of one focused delegated subtask.",
    prompt:
      "Complete the delegated subtask autonomously. Keep the scope focused, inspect before editing, verify any changes, and return a concise evidence-first result.",
    model: "inherit",
  },
  reviewer: {
    description: "High-confidence review for correctness, security, and missing tests.",
    prompt:
      "Review the requested code or diff. Report only concrete, actionable findings with path and line evidence; prioritize correctness and security over style.",
    model: "inherit",
  },
}

const cursorBoundary = `Cursor provider boundary:
- You are the Cursor local agent running inside OpenCode. Execute work directly with Cursor's native tools; do not wait for OpenCode to execute tool calls for you.
- Cursor's shell, read, write, edit, delete, MCP, and subagent calls run outside OpenCode's permission prompts. Follow the user's stated limits, never broaden access, and fail closed when authorization is ambiguous.
- OpenCode tool names in the surrounding instructions mean the equivalent Cursor-native operation.
- When a listed oy skill matches the task, read .cursor/skills/<id>/SKILL.md before starting it.`

export const bundledSkillsCatalogue = [
  "<available_cursor_skills>",
  "Load the matching skill from `.cursor/skills/<id>/SKILL.md` before starting relevant work:",
  "",
  ...bundledCursorSkills.map((skill) => `- **${skill.id}**: ${skill.description}`),
  "</available_cursor_skills>",
].join("\n")

const cursorOptions = (options) => ({
  ...(options ?? {}),
  session: options?.session ?? "auto",
  settingSources: options?.settingSources ?? ["project"],
  agents: { ...defaultCursorAgents, ...(options?.agents ?? {}) },
  skillsCatalogue: [bundledSkillsCatalogue, options?.skillsCatalogue].filter(Boolean).join("\n\n"),
})

const sessionHeader = (headers) => {
  for (const [name, value] of Object.entries(headers ?? {})) {
    if (["x-session-id", "x-session-affinity", "x-opencode-session"].includes(name.toLowerCase())) {
      return value
    }
  }
}

const callOptions = (options, providerName) => {
  const existing = options.providerOptions?.[providerName] ?? {}
  const sessionID = existing.sessionID ?? sessionHeader(options.headers)
  const ephemeral = existing.ephemeral ?? (!Array.isArray(options.tools) || options.tools.length === 0)
  return {
    ...options,
    prompt: [
      { role: "system", content: cursorBoundary },
      ...options.prompt,
    ],
    providerOptions: {
      ...(options.providerOptions ?? {}),
      [providerName]: {
        ...existing,
        ...(sessionID ? { sessionID } : {}),
        ...(ephemeral ? { ephemeral: true } : {}),
      },
    },
  }
}

const wrapLanguageModel = (language, providerName) =>
  new Proxy(language, {
    get(target, property) {
      if (property === "doGenerate") {
        return (options) => target.doGenerate(callOptions(options, providerName))
      }
      if (property === "doStream") {
        return (options) => target.doStream(callOptions(options, providerName))
      }
      const value = Reflect.get(target, property, target)
      return typeof value === "function" ? value.bind(target) : value
    },
  })

export const createQualityCursor = (createCursor, options) => {
  const configured = cursorOptions(options)
  const provider = createCursor(configured)
  const providerName = configured.name ?? "cursor"
  return new Proxy(provider, {
    get(target, property) {
      if (property === "languageModel") {
        return (modelID) => wrapLanguageModel(target.languageModel(modelID), providerName)
      }
      const value = Reflect.get(target, property, target)
      return typeof value === "function" ? value.bind(target) : value
    },
  })
}
