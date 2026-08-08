import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

import { safeWorkspacePath } from "./cursor-paths.js"
import { bundledCursorSkills } from "./cursor-provider.js"

const sentinel = "generated: oy-cursor-skill"
const ignoreStart = "# oy cursor skills start"
const ignoreEnd = "# oy cursor skills end"

const isGenerated = (content) => {
  if (!content.startsWith("---")) return false
  const end = content.indexOf("\n---", 3)
  return (end === -1 ? content : content.slice(0, end)).split(/\r?\n/).includes(sentinel)
}

const stamp = (content) => {
  if (!content.startsWith("---")) return `---\n${sentinel}\n---\n\n${content}`
  const end = content.indexOf("\n---", 3)
  if (end === -1 || content.slice(0, end).includes(sentinel)) return content
  return `${content.slice(0, end)}\n${sentinel}${content.slice(end)}`
}

const ignoreBlock = [
  ignoreStart,
  ...bundledCursorSkills.map((skill) => `${skill.id}/`),
  ".gitignore",
  ignoreEnd,
].join("\n")

export const writeBundledCursorSkills = (directory, assets, warn = console.warn) => {
  let root
  try {
    root = safeWorkspacePath(directory, ".cursor", "skills")
  } catch (error) {
    warn(`[oy] refusing to mirror Cursor skills: ${error instanceof Error ? error.message : String(error)}`)
    return
  }
  for (const skill of bundledCursorSkills) {
    const source = join(assets, "skills", skill.id, "SKILL.md")
    try {
      const destination = safeWorkspacePath(directory, ".cursor", "skills", skill.id, "SKILL.md")
      const existing = existsSync(destination) ? readFileSync(destination, "utf8") : undefined
      if (existing !== undefined && !isGenerated(existing)) {
        warn(`[oy] preserving user-owned .cursor/skills/${skill.id}/SKILL.md`)
        continue
      }
      const content = stamp(readFileSync(source, "utf8"))
      if (existing === content) continue
      mkdirSync(dirname(destination), { recursive: true })
      writeFileSync(destination, content, "utf8")
    } catch (error) {
      warn(`[oy] failed to mirror Cursor skill ${skill.id}: ${error instanceof Error ? error.message : String(error)}`)
    }
  }
  try {
    const ignore = safeWorkspacePath(directory, ".cursor", "skills", ".gitignore")
    const existing = existsSync(ignore) ? readFileSync(ignore, "utf8") : ""
    if (!existing.includes(ignoreStart)) {
      mkdirSync(root, { recursive: true })
      const prefix = existing && !existing.endsWith("\n") ? `${existing}\n` : existing
      writeFileSync(ignore, `${prefix}${ignoreBlock}\n`, "utf8")
    }
  } catch (error) {
    warn(`[oy] failed to ignore mirrored Cursor skills: ${error instanceof Error ? error.message : String(error)}`)
  }
}

export const removeBundledCursorSkills = (directory) => {
  for (const skill of bundledCursorSkills) {
    try {
      const path = safeWorkspacePath(directory, ".cursor", "skills", skill.id, "SKILL.md")
      if (isGenerated(readFileSync(path, "utf8"))) rmSync(dirname(path), { recursive: true, force: true })
    } catch {
      // The mirror may not exist or may be user-owned.
    }
  }
  try {
    const path = safeWorkspacePath(directory, ".cursor", "skills", ".gitignore")
    const content = readFileSync(path, "utf8")
    const start = content.indexOf(ignoreStart)
    const end = content.indexOf(ignoreEnd, start)
    if (start === -1 || end === -1) return
    const next = `${content.slice(0, start)}${content.slice(end + ignoreEnd.length)}`.replace(/^\n+|\n+$/g, "")
    if (next) writeFileSync(path, `${next}\n`, "utf8")
    else rmSync(path)
  } catch {
    // Best-effort cleanup.
  }
}
