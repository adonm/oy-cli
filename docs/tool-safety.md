# Tool safety

`oy` tools run on the user's machine, inside the configured workspace unless noted. Tools are not a sandbox; they use the current user permissions and may expose snippets or command output to the model transcript.

## Capability matrix

| Tool | Capability | Mutation | Main gate | Notes |
|---|---|---:|---|---|
| `list` | Lists workspace paths by glob | No | Always available | Use first for discovery |
| `read` | Reads one UTF-8 workspace file | No | Always available | Prefer narrow `offset`/`limit` slices |
| `search` | Searches workspace text | No | Always available | Prefer literal mode for exact strings |
| `sloc` | Counts source lines with tokei | No | Always available | Useful for sizing and planning |
| `todo` | Manages in-memory todos | No by default | Always available | `persist=true` writes `TODO.md` and uses write approval |
| `ask` | Asks the user in interactive runs | No | Interactive only | Use only for genuine ambiguity |
| `webfetch` | Fetches public web pages/files | No local mutation | Network policy | Blocks sensitive headers and non-public targets by validation |
| `replace` | Replaces text in workspace files | Yes | File-write approval | Inspect/search before changing |
| `bash` | Runs a shell command in workspace | Process side effects | Shell approval | Inherits environment and user permissions |

## Approval modes

| Mode | File writes | Shell | Intended use |
|---|---:|---:|---|
| `default` / `ask` | Ask | Ask | Normal trusted work |
| `plan` / `read` | Deny | Deny | First look or untrusted repository |
| `accept-edits` / `edit` | Auto | Ask | Trusted mechanical edits |
| `auto-approve` / `auto` | Auto | Auto | Trusted unattended work only |

Network availability is controlled separately from file-write and shell approval. Treat any mode with both workspace reads and network fetches as able to disclose workspace content if the model is confused by untrusted instructions.

## Filesystem boundary

Workspace tools should only operate within `OY_ROOT` or the current directory. When editing this boundary:

- reject absolute paths and parent traversal where appropriate,
- canonicalize existing paths,
- check symlink ancestors and final destinations,
- keep final writes inside the workspace,
- add tests for traversal, symlinks, and missing parents.

## Network boundary

`webfetch` is for public documentation and public API research. It follows redirects by default and sends an honest `oy-cli/<version>` `User-Agent` plus document-friendly `Accept` headers so common docs URLs work without model-supplied header tuning. It should still fail closed for localhost, private, link-local, reserved, multicast, and ambiguous address forms. When changing it:

- validate before each request and redirect,
- keep redirects capped and public-only,
- normalize IPv4-mapped IPv6 addresses,
- reject sensitive request headers,
- keep default headers non-credentialed and overrideable only through validation,
- cap time and response size,
- add regression tests for public/private IP classification and webfetch defaults.

## Shell boundary

`bash` is the highest-risk tool. It can read credentials, modify files, contact networks, start processes, and affect the host outside the repo. Keep shell use explicit:

- ask by default,
- deny in read-only modes,
- include the command in the approval preview,
- prefer file tools for inspection and small edits,
- avoid destructive commands unless explicitly requested.

## Audit disclosure boundary

`oy audit` has no model tools, but it sends collected file text to the model provider. Audit collection should skip build outputs, dependencies, lockfiles, hidden or likely-secret files by default, and should explain any option that includes more data.
