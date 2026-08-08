import { lstatSync } from "node:fs"
import { isAbsolute, join, relative, resolve, sep } from "node:path"

export const safeWorkspacePath = (directory, ...parts) => {
  const root = resolve(directory)
  const target = resolve(root, ...parts)
  const scoped = relative(root, target)
  if (scoped === ".." || scoped.startsWith(`..${sep}`) || isAbsolute(scoped)) {
    throw new Error(`path escapes workspace: ${target}`)
  }

  let current = root
  for (const part of scoped.split(sep).filter(Boolean)) {
    current = join(current, part)
    try {
      if (lstatSync(current).isSymbolicLink()) {
        throw new Error(`refusing symlinked workspace path: ${current}`)
      }
    } catch (error) {
      if (error?.code === "ENOENT") break
      throw error
    }
  }
  return target
}
