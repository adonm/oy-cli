import assert from "node:assert/strict"
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import test from "node:test"

import { removeBundledCursorSkills, writeBundledCursorSkills } from "../cursor-skills.js"

const assets = new URL("../assets", import.meta.url).pathname

test("mirrors bundled skills for Cursor and cleans up only generated copies", () => {
  const directory = mkdtempSync(join(process.cwd(), ".cursor-skills-test-"))
  try {
    writeBundledCursorSkills(directory, assets, assert.fail)
    const skill = join(directory, ".cursor", "skills", "oy-audit", "SKILL.md")
    assert.equal(existsSync(skill), true)
    assert.match(readFileSync(skill, "utf8"), /generated: oy-cursor-skill/)
    assert.match(readFileSync(join(directory, ".cursor", "skills", ".gitignore"), "utf8"), /oy-audit\//)

    removeBundledCursorSkills(directory)
    assert.equal(existsSync(skill), false)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("preserves a user-owned Cursor skill", () => {
  const directory = mkdtempSync(join(process.cwd(), ".cursor-skills-user-test-"))
  const skill = join(directory, ".cursor", "skills", "oy-audit", "SKILL.md")
  mkdirSync(join(directory, ".cursor", "skills", "oy-audit"), { recursive: true })
  writeFileSync(skill, "---\nname: oy-audit\n---\n\nUser owned\n")
  try {
    writeBundledCursorSkills(directory, assets, () => {})
    removeBundledCursorSkills(directory)
    assert.match(readFileSync(skill, "utf8"), /User owned/)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("refuses symlinked Cursor skill ancestors during writes and cleanup", () => {
  const cases = [
    { link: [".cursor"], escaped: ["skills", "oy-audit", "SKILL.md"] },
    { link: [".cursor", "skills"], escaped: ["oy-audit", "SKILL.md"] },
    { link: [".cursor", "skills", "oy-audit"], escaped: ["SKILL.md"] },
  ]
  for (const scenario of cases) {
    const directory = mkdtempSync(join(process.cwd(), ".cursor-skills-boundary-test-"))
    const outside = mkdtempSync(join(process.cwd(), ".cursor-skills-outside-test-"))
    const link = join(directory, ...scenario.link)
    const sentinel = join(outside, "sentinel")
    mkdirSync(dirname(link), { recursive: true })
    writeFileSync(sentinel, "keep")
    symlinkSync(outside, link, "dir")
    const warnings = []
    try {
      writeBundledCursorSkills(directory, assets, (warning) => warnings.push(warning))
      removeBundledCursorSkills(directory)
      assert.equal(readFileSync(sentinel, "utf8"), "keep")
      assert.equal(existsSync(join(outside, ...scenario.escaped)), false)
      assert.ok(warnings.some((warning) => warning.includes("symlinked workspace path")))
    } finally {
      rmSync(directory, { recursive: true, force: true })
      rmSync(outside, { recursive: true, force: true })
    }
  }
})
